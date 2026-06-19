//! `mde-music` binary — AIR-10/11 shell.
//!
//! Renders the 7-card library hub + a breadcrumb the user navigates,
//! plus an Airsonic connection banner (from the shared creds). The live
//! grids behind each card + playback land with the `mde-musicd` data
//! path (AIR-10.b / AIR-2); this shell is the §0.12 runtime-reachable
//! entry point that makes the [`hub`]/[`nav`] models live.
//!
//! EFF-34 — ported off iced 0.13 onto libcosmic's vendored iced. `cosmic::iced`
//! is the fork; the shell is a `cosmic::Application`, and the per-widget style
//! closures bridge to cosmic's class-based theming via [`cosmic_compat`].

mod cosmic_compat;
use cosmic_compat::{ButtonSty, TextSty};

use cosmic::iced::widget::{
    button, column, container, image, row, scrollable, stack, text, text_input, Space,
};
use cosmic::iced::{Length, Subscription};
use cosmic::{Application, ApplicationExt, Element};

use mde_music::album::{self, AlbumView};
use mde_music::color;
use mde_music::hub::HubCard;
use mde_music::library::{self, LibraryItem};
use mde_music::nav::{NavState, Route};
use mde_music::nowplaying::{self, NowState};
use mde_music::prefs::{self, SortKey};
use mde_music::search::{self, SearchResults};
use mde_musicd::creds::{self, Creds};

/// The reducer's iced-space [`Task`] (the inherent `update`/subscription build
/// these; the `cosmic::Application` trait lifts them into the cosmic Action
/// space). Aliased to keep the AIR-* call sites reading as plain `Task`.
type Task<M> = cosmic::iced::Task<M>;

fn main() -> cosmic::iced::Result {
    cosmic::app::run::<State>(cosmic::app::Settings::default(), ())
}

/// MUSIC-RFX-5 — run a queue-mutation (an RFX-1 daemon verb) then re-fetch the
/// queue so the maxi Queue tab reflects the new order/contents. The mutation's
/// result is ignored (a failed verb just leaves the queue unchanged on reload).
fn reload_queue_after(
    mutation: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
) -> cosmic::iced::Task<Message> {
    cosmic::iced::Task::perform(
        async move {
            let _ = mutation.await;
            nowplaying::fetch_queue().await
        },
        |r| match r {
            Ok((songs, current)) => Message::QueueLoaded(songs, current),
            Err(_) => Message::QueueLoaded(Vec::new(), 0),
        },
    )
}

/// MUSIC-RFX-6 — run a playlist edit (an RFX-3 daemon verb) then re-fetch the
/// playlists list so the Playlists hub card reflects the change live.
fn reload_playlists_after(
    op: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
) -> cosmic::iced::Task<Message> {
    cosmic::iced::Task::perform(
        async move {
            let _ = op.await;
            library::fetch("list-playlists").await
        },
        |r| match r {
            Ok(items) => Message::ItemsLoaded(items),
            Err(e) => Message::ItemsFailed(e),
        },
    )
}

/// Convert an mde-theme Carbon token (`Rgba`, u8 channels) to this crate's
/// `iced::Color` at alpha `a`. mde-music's iced version skews from the one
/// `mde_theme::into_iced_color()` targets, so the conversion is by hand — the
/// single sanctioned spot for raw channel math, keeping every call site on a
/// token rather than a literal (§4).
fn carbon(rgba: mde_theme::Rgba, a: f32) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a,
    }
}

/// MUSIC-HOME-2 — group a count with thousands separators (23126 → "23,126").
fn commafy(n: u64) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Build a Carbon `cosmic::iced::Theme` from the canonical `mde_theme::Palette`
/// (E5.3) — the Q2 indigo accent, Apple-charcoal background, and the semantic
/// tokens, single-sourced from the design palette.
///
/// EFF-34 — under the `cosmic::Application` shell the global theme is cosmic's
/// (the `.theme(..)` builder of the old `iced::application` is gone), so this is
/// no longer wired into the runtime; it's retained as the **§4 token-derivation
/// guard** exercised by [`theme_tests`] (a future per-widget Carbon pass, à la
/// mde-files, would consume the same palette). Hence `dead_code` outside tests.
#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
fn mde_music_iced_theme() -> cosmic::iced::Theme {
    use mde_theme::Palette;
    // Opaque conversion of an mde_theme token — delegates to the module-level
    // `carbon` helper (the one place channel math lives).
    fn c(rgba: mde_theme::Rgba) -> cosmic::iced::Color {
        carbon(rgba, 1.0)
    }
    let p = Palette::dark();
    // EFF-34 — libcosmic's vendored iced `theme::Palette` carries the
    // `warning` role (unlike the crates.io iced 0.13 this crate used to
    // target), so seed it from the Carbon warning token alongside the rest.
    let palette = cosmic::iced::theme::Palette {
        background: c(p.background),
        text: c(p.text),
        primary: c(p.accent),
        success: c(p.success),
        warning: c(p.warning),
        danger: c(p.danger),
    };
    cosmic::iced::Theme::custom("MDE Music".to_string(), palette)
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
    /// EFF-34 — the `cosmic::Application` shell state (window/theme/a11y).
    core: cosmic::app::Core,
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
    /// MUSIC-RESPONSIVE-1 — guards the one-shot auto-retry: a browse fetch that
    /// times out (the daemon briefly busy / not yet warm after launch) retries
    /// itself once before surfacing the error, so a cold-boot race or a slow
    /// first Airsonic call recovers instead of dead-ending the view. Reset on
    /// every fresh navigation + on a successful load.
    load_retried: bool,
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
    /// MUSIC-HOME-2 — the server library stats shown on the Home page (`None`
    /// until the first `library-stats` fetch returns).
    stats: Option<library::LibraryStats>,
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
    /// MUSIC-RFX-4 — the now-playing track's dominant cover colour, for the maxi
    /// art tint (distinct from `album_color`, which tracks the opened album view).
    now_color: (u8, u8, u8),
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
    /// MUSIC-LOCK-FIX — ids whose cover-art fetch was already issued (so the
    /// scroll-window loader doesn't re-fetch the same card per scroll event).
    /// Reset when the item set changes (a new category load).
    art_requested: std::collections::HashSet<String>,
    /// AIR-15.b — maxi-player (full-window) open flag + its queue snapshot.
    maxi_open: bool,
    queue_songs: Vec<String>,
    queue_current: usize,
    /// Resolved queue song-id -> title (fan-out via get-song).
    queue_titles: std::collections::HashMap<String, String>,
    /// MUSIC-RFX-5 — the multi-selected queue row indices (for "remove selected").
    /// Cleared on any structural mutation since indices shift after a reorder/remove.
    queue_selected: std::collections::HashSet<usize>,
    /// MUSIC-RFX-7 — the "add to playlist" picker: the song id pending add
    /// (`Some` = the picker sheet is open) + the playlist choices loaded for it.
    add_to_playlist_song: Option<String>,
    add_to_playlist_choices: Vec<(String, String)>,
    /// MUSIC-RFX-6 — the "new playlist" name input on the Playlists page.
    new_playlist_name: String,
    /// MUSIC-RFX-6 — the playlist id currently being renamed inline (+ its buffer);
    /// `None` when no rename is open.
    renaming_playlist: Option<String>,
    rename_buffer: String,
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
    /// MUSIC-HOME-2 — the server library-stats snapshot returned.
    StatsLoaded(Result<library::LibraryStats, String>),
    /// MUSIC-NAV — go up one breadcrumb level (Back).
    Back,
    /// MUSIC-NAV — jump to the Library root (Home).
    Home,
    /// MUSIC-NAV — quit the app (the window has no title-bar chrome).
    Exit,
    /// A category fetch resolved.
    ItemsLoaded(Vec<LibraryItem>),
    /// A category fetch failed (daemon down / no server).
    ItemsFailed(String),
    /// MUSIC-RESPONSIVE-1 — re-issue the current route's grid fetch (the
    /// one-shot auto-retry after a timeout).
    RetryLoad,
    /// AIR-11.c — the window resized; updates the adaptive-grid column count.
    WindowResized(f32),
    /// AIR-11.c.2 — the library grid scrolled; record the offset per route.
    GridScrolled(f32),
    /// AIR-11.c.3 — a grid card's cover art fetched (id, decoded handle or None).
    ArtLoaded(String, Option<image::Handle>),
    /// AIR-15.b — toggle the maxi-player full-window surface.
    ToggleMaxi,
    /// MUSIC-PLAYBAR — open the full view at the Peers tab (audio routing).
    OpenRouting,
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
    /// MUSIC-RFX-5 — queue management (all via the RFX-1 daemon verbs).
    QueueRemove(usize),
    QueuePlayNext(usize),
    QueueMoveUp(usize),
    QueueMoveDown(usize),
    QueueToggleSelect(usize),
    QueueRemoveSelected,
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
    /// MUSIC-RFX-7 — add-to-playlist picker.
    OpenAddToPlaylist(String),
    AddPlaylistChoicesLoaded(Vec<(String, String)>),
    AddSongToPlaylist(String),
    CloseAddToPlaylist,
    /// MUSIC-RFX-6 — playlist editor (create / rename / delete).
    NewPlaylistNameChanged(String),
    CreatePlaylist,
    StartRenamePlaylist(String, String),
    RenameBufferChanged(String),
    CommitRenamePlaylist,
    CancelRenamePlaylist,
    DeletePlaylist(String),
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
    NowArtReady(Option<image::Handle>, (u8, u8, u8)),
    /// AIR-15.b.3 — the maxi volume slider changed.
    SetVolume(f32),
    /// MUSIC-RFX-4 — the maxi scrub slider moved (target position, ms).
    Seek(u64),
    PlayPause,
    SkipNext,
    SkipPrev,
    /// A transport command (volume/play/skip) finished — outcome is
    /// irrelevant; the follow-up state fetch is the truth (sweep-3 I8
    /// dropped the never-read `Result` payload).
    TransportDone,
}

impl State {
    fn new(core: cosmic::app::Core) -> Self {
        let (form, connection) = match creds::load() {
            Ok(c) => (None, format!("Connected to {}", c.server_url)),
            Err(_) => (Some(FirstRunForm::default()), String::new()),
        };
        Self {
            core,
            nav: NavState::new(),
            form,
            connection,
            items: Vec::new(),
            loading: false,
            load_error: None,
            load_retried: false,
            search_query: String::new(),
            search_seq: 0,
            search_results: None,
            searching: false,
            search_error: None,
            search_open: false,
            album: None,
            album_loading: false,
            album_error: None,
            stats: None,
            now_state: NowState::default(),
            now_title: String::new(),
            now_artist: String::new(),
            now_art: None,
            now_duration_ms: 0,
            album_color: color::accent_rgb(),
            album_text_color: (255, 255, 255),
            now_color: color::accent_rgb(),
            album_art: None,
            sort: prefs::load().sort,
            grid_width: 1100.0,
            grid_scroll: prefs::load().scroll.into_iter().collect(),
            art_cache: std::collections::HashMap::new(),
            art_requested: std::collections::HashSet::new(),
            maxi_open: false,
            queue_songs: Vec::new(),
            queue_current: 0,
            queue_titles: std::collections::HashMap::new(),
            queue_selected: std::collections::HashSet::new(),
            add_to_playlist_song: None,
            add_to_playlist_choices: Vec::new(),
            new_playlist_name: String::new(),
            renaming_playlist: None,
            rename_buffer: String::new(),
            maxi_tab: MaxiTab::Queue,
            maxi_lyrics: Vec::new(),
            maxi_peers: Vec::new(),
        }
    }

    /// MUSIC-RESPONSIVE-1 — re-issue the grid fetch for the current route (used
    /// by the one-shot timeout auto-retry). Mirrors the per-route fetch the
    /// Open* handlers dispatch; non-grid routes (Hub/Album/Search) have nothing
    /// to reload.
    fn reload_current(&self) -> Task<Message> {
        let to_msg = |r: Result<Vec<library::LibraryItem>, String>| match r {
            Ok(items) => Message::ItemsLoaded(items),
            Err(e) => Message::ItemsFailed(e),
        };
        match self.nav.current() {
            Route::Category(card) => match library::verb_for(*card) {
                Some(verb) => Task::perform(library::fetch(verb), to_msg),
                None => Task::none(),
            },
            Route::Artist(id, _) => {
                Task::perform(library::fetch_albums_by_artist(id.clone()), to_msg)
            }
            Route::Genre(g) => Task::perform(library::fetch_albums_by_genre(g.clone()), to_msg),
            Route::Podcast(id, _) => {
                Task::perform(library::fetch_podcast_episodes(id.clone()), to_msg)
            }
            _ => Task::none(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenCard(card) => {
                self.nav.push(Route::Category(card));
                self.items.clear();
                self.load_error = None;
                self.load_retried = false;
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
                self.load_retried = false;
                // AIR-11.c.2 — restore this category's saved scroll offset.
                let y = self
                    .grid_scroll
                    .get(&self.nav.current().segment())
                    .copied()
                    .unwrap_or(0.0);
                let restore = cosmic::iced::widget::scrollable::scroll_to(
                    grid_scroll_id(),
                    // EFF-34 — the fork's `AbsoluteOffset` axes are `Option<f32>`
                    // (a per-axis "leave as-is" None); pin y, leave x untouched.
                    cosmic::iced::widget::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(y),
                    },
                );
                // MUSIC-LOCK-FIX (2026-06-18) — a large library (hundreds of
                // artists) used to fan out one cover-art fetch per card; each
                // completion re-renders the WHOLE grid, so art loading was O(N²)
                // re-renders and the UI locked. Now only the visible scroll window
                // fetches art (bounded), restored to this category's saved offset.
                self.art_requested.clear();
                let art = self.art_window_task(y);
                Task::batch([restore, art])
            }
            Message::ItemsFailed(e) => {
                // MUSIC-RESPONSIVE-1 — a timeout ("daemon not responding") right
                // after launch is usually the daemon not yet warm / briefly busy,
                // not a real outage (the daemon answers in <1s once ready). Retry
                // the current route's fetch once before surfacing the error, so
                // the cold-boot race recovers silently instead of dead-ending.
                let timed_out = e.contains("daemon not responding");
                if timed_out && !self.load_retried {
                    self.load_retried = true;
                    // Stay in the loading state; retry after a short settle.
                    return Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                        },
                        |()| Message::RetryLoad,
                    );
                }
                self.items.clear();
                self.loading = false;
                self.load_error = Some(e);
                Task::none()
            }
            Message::RetryLoad => {
                self.load_error = None;
                self.loading = true;
                self.reload_current()
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
            // MUSIC-PLAYBAR — the playback bar's audio-routing control: open the
            // full view on the Peers tab (the AIR-8 take-over routing surface).
            Message::OpenRouting => {
                self.maxi_open = true;
                self.maxi_tab = MaxiTab::Peers;
                return Task::perform(nowplaying::fetch_peer_states(), |r| {
                    Message::PeersLoaded(r.unwrap_or_default())
                });
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
            // MUSIC-RFX-5 — each mutation drives the RFX-1 daemon verb then
            // re-fetches the queue (indices shift, so the selection is cleared).
            Message::QueueRemove(idx) => {
                self.queue_selected.clear();
                reload_queue_after(nowplaying::queue_remove(idx))
            }
            Message::QueuePlayNext(idx) => {
                self.queue_selected.clear();
                reload_queue_after(nowplaying::queue_move_to_next(idx))
            }
            Message::QueueMoveUp(idx) => {
                self.queue_selected.clear();
                if idx == 0 {
                    Task::none()
                } else {
                    reload_queue_after(nowplaying::queue_move(idx, idx - 1))
                }
            }
            Message::QueueMoveDown(idx) => {
                self.queue_selected.clear();
                reload_queue_after(nowplaying::queue_move(idx, idx + 1))
            }
            Message::QueueToggleSelect(idx) => {
                if !self.queue_selected.remove(&idx) {
                    self.queue_selected.insert(idx);
                }
                Task::none()
            }
            Message::QueueRemoveSelected => {
                let mut idxs: Vec<usize> = self.queue_selected.drain().collect();
                idxs.sort_unstable();
                if idxs.is_empty() {
                    Task::none()
                } else {
                    reload_queue_after(nowplaying::queue_remove_many(idxs))
                }
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
            Message::Back => {
                // Up one breadcrumb level; at root this is a no-op.
                let depth = self.nav.breadcrumb().len();
                if depth > 1 {
                    self.nav.ascend_to(depth - 2);
                }
                Task::none()
            }
            Message::Home => {
                self.nav.ascend_to(0);
                // MUSIC-HOME-2 — (re)load the server stats for the Home dashboard.
                Task::perform(library::fetch_library_stats(), Message::StatsLoaded)
            }
            Message::StatsLoaded(r) => {
                if let Ok(s) = r {
                    self.stats = Some(s);
                }
                Task::none()
            }
            Message::Exit => std::process::exit(0),
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
                cosmic::widget::text_input::focus(search_id())
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
                // Artist browse — load the artist's albums into the grid (was a
                // no-op: it pushed the breadcrumb but never loaded the next layer).
                self.nav.push(Route::Artist(id.clone(), name));
                self.dismiss_search();
                self.items.clear();
                self.load_error = None;
                self.load_retried = false;
                self.loading = true;
                Task::perform(library::fetch_albums_by_artist(id), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                })
            }
            Message::OpenGenre(genre) => {
                self.nav.push(Route::Genre(genre.clone()));
                self.dismiss_search();
                self.items.clear();
                self.load_error = None;
                self.load_retried = false;
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
                self.load_retried = false;
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
            // MUSIC-RFX-7 — add-to-playlist picker (reachable from any track row).
            Message::OpenAddToPlaylist(song_id) => {
                self.add_to_playlist_song = Some(song_id);
                self.add_to_playlist_choices.clear();
                Task::perform(library::fetch("list-playlists"), |r| match r {
                    Ok(items) => Message::AddPlaylistChoicesLoaded(
                        items.into_iter().map(|i| (i.id, i.label)).collect(),
                    ),
                    Err(_) => Message::AddPlaylistChoicesLoaded(Vec::new()),
                })
            }
            Message::AddPlaylistChoicesLoaded(choices) => {
                self.add_to_playlist_choices = choices;
                Task::none()
            }
            Message::AddSongToPlaylist(playlist_id) => {
                let song = self.add_to_playlist_song.take();
                self.add_to_playlist_choices.clear();
                match song {
                    Some(s) => Task::perform(
                        album::playlist_add_track(playlist_id, s),
                        Message::AlbumActionDone,
                    ),
                    None => Task::none(),
                }
            }
            Message::CloseAddToPlaylist => {
                self.add_to_playlist_song = None;
                self.add_to_playlist_choices.clear();
                Task::none()
            }
            // MUSIC-RFX-6 — playlist editor. Each op drives the RFX-3 verb then
            // re-fetches the playlists list so the hub card reflects it live.
            Message::NewPlaylistNameChanged(s) => {
                self.new_playlist_name = s;
                Task::none()
            }
            Message::CreatePlaylist => {
                let name = self.new_playlist_name.trim().to_string();
                if name.is_empty() {
                    Task::none()
                } else {
                    self.new_playlist_name.clear();
                    reload_playlists_after(album::playlist_create(name))
                }
            }
            Message::StartRenamePlaylist(id, current) => {
                self.renaming_playlist = Some(id);
                self.rename_buffer = current;
                Task::none()
            }
            Message::RenameBufferChanged(s) => {
                self.rename_buffer = s;
                Task::none()
            }
            Message::CommitRenamePlaylist => {
                let name = self.rename_buffer.trim().to_string();
                match self.renaming_playlist.take() {
                    Some(id) if !name.is_empty() => {
                        reload_playlists_after(album::playlist_rename(id, name))
                    }
                    _ => Task::none(),
                }
            }
            Message::CancelRenamePlaylist => {
                self.renaming_playlist = None;
                Task::none()
            }
            Message::DeletePlaylist(id) => {
                if self.renaming_playlist.as_deref() == Some(id.as_str()) {
                    self.renaming_playlist = None;
                }
                reload_playlists_after(album::playlist_delete(id))
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
                // MUSIC-LOCK-FIX — load cover art for the newly-visible window.
                self.art_window_task(y)
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
                    // MUSIC-RFX-4 — fetch the cover + extract its dominant colour
                    // for the maxi art tint (same path as the album view's art).
                    Some(c) => Task::perform(color::fetch_cover_art(c), |r| match r {
                        Ok(bytes) if !bytes.is_empty() => {
                            let handle = image::Handle::from_bytes(bytes.clone());
                            let (d, _t) = color::extract(&bytes)
                                .unwrap_or((color::accent_rgb(), (255, 255, 255)));
                            Message::NowArtReady(Some(handle), d)
                        }
                        _ => Message::NowArtReady(None, color::accent_rgb()),
                    }),
                    None => Task::none(),
                }
            }
            Message::NowArtReady(handle, dominant) => {
                self.now_art = handle;
                self.now_color = dominant;
                Task::none()
            }
            Message::SetVolume(v) => {
                self.now_state.volume = v;
                Task::perform(nowplaying::set_volume(v), |_| Message::TransportDone)
            }
            // MUSIC-RFX-4 — scrub: jump the playhead optimistically so the slider
            // tracks the drag, then tell the daemon to seek (RFX-2).
            Message::Seek(ms) => {
                self.now_state.position_ms = ms;
                Task::perform(nowplaying::seek(ms), |_| Message::TransportDone)
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
        // EFF-34 — the fork's keyboard facade has no `on_key_press`; the raw
        // `KeyPressed` event (via `listen_with`) drives the same shortcuts.
        let keys = cosmic::iced::event::listen_with(|event, _status, _id| {
            use cosmic::iced::keyboard::key::Named;
            use cosmic::iced::keyboard::{Event as Kbd, Key};
            let cosmic::iced::Event::Keyboard(Kbd::KeyPressed { key, modifiers, .. }) = event
            else {
                return None;
            };
            match key.as_ref() {
                Key::Character("f") if modifiers.command() => Some(Message::FocusSearch),
                Key::Named(Named::Escape) => Some(Message::DismissSearch),
                _ => None,
            }
        });
        // AIR-11.c — track window width so the library grid can reflow
        // its columns (the facade has no `responsive`; the resize event
        // drives the adaptive layout instead).
        let resizes = cosmic::iced::event::listen_with(|event, _status, _id| match event {
            cosmic::iced::Event::Window(cosmic::iced::window::Event::Resized(size)) => {
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
                cosmic::iced::time::every(nowplaying::POLL).map(|_| Message::PollState),
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
            Space::new().height(Length::Fixed(8.0)),
            text("Point MDE Music at your Airsonic / Navidrome server.").size(13),
            Space::new().height(Length::Fixed(16.0)),
            text_input("https://music.your-mesh:4040", &f.url).on_input(Message::UrlChanged),
            text_input("username", &f.user).on_input(Message::UserChanged),
            text_input("password", &f.pass)
                .secure(true)
                .on_input(Message::PassChanged),
            Space::new().height(Length::Fixed(12.0)),
            button(text("Connect")).on_press(Message::Connect),
        ]
        .spacing(8)
        .padding(28)
        .max_width(440);
        if let Some(err) = &f.error {
            col = col.push(Space::new().height(Length::Fixed(8.0)));
            col = col.push(text(err.clone()).size(13));
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// MUSIC-LOCK-FIX — fetch cover art only for the grid window around scroll
    /// offset `offset_y`, bounded so a large library can't fan out hundreds of
    /// fetches (each of which re-renders the whole grid → the UI lock). Skips
    /// already-cached + already-requested ids; marks the rest requested. The
    /// row pitch (168) + column count mirror the grid in [`Self::library_view`].
    fn art_window_task(&mut self, offset_y: f32) -> Task<Message> {
        const ROW_PITCH: f32 = 168.0; // 160px card + 8px spacing
        const WINDOW_ROWS: usize = 14; // ~2 screenfuls + buffer
        let cols = ((self.grid_width + 8.0) / ROW_PITCH).floor().max(1.0) as usize;
        let start_row = ((offset_y / ROW_PITCH).floor() as usize).saturating_sub(2);
        let start = start_row.saturating_mul(cols);
        let end = (start + WINDOW_ROWS * cols).min(self.items.len());
        if start >= end {
            return Task::none();
        }
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for it in &self.items[start..end] {
            if self.art_cache.contains_key(&it.id) || self.art_requested.contains(&it.id) {
                continue;
            }
            let Some(aid) = it.art_id.clone() else {
                continue;
            };
            self.art_requested.insert(it.id.clone());
            let id = it.id.clone();
            tasks.push(Task::perform(color::fetch_cover_art(aid), move |r| {
                Message::ArtLoaded(id.clone(), r.ok().map(|b| image::Handle::from_bytes(b)))
            }));
        }
        Task::batch(tasks)
    }

    /// MUSIC-ALBUMS-2 — the Carbon left sidebar (256px): Home · Internet Radio ·
    /// LIBRARY (Albums/Artists/Genres/Playlists/Podcasts) · Recently Added ·
    /// Settings. The active item carries the accent rail + raised fill. Routes
    /// through the existing nav messages — no new backend.
    fn carbon_sidebar(&self) -> Element<'_, Message> {
        let p = mde_theme::Palette::dark();
        let cur = self.nav.current();
        let cat_active = |c: HubCard| match c {
            HubCard::Albums => matches!(cur, Route::Category(HubCard::Albums) | Route::Album(..)),
            HubCard::Artists => {
                matches!(cur, Route::Category(HubCard::Artists) | Route::Artist(..))
            }
            HubCard::Genres => matches!(cur, Route::Category(HubCard::Genres) | Route::Genre(..)),
            HubCard::Podcasts => {
                matches!(cur, Route::Category(HubCard::Podcasts) | Route::Podcast(..))
            }
            other => matches!(cur, Route::Category(x) if *x == other),
        };
        let mut col = column![]
            .spacing(2)
            .width(Length::Fixed(256.0))
            .padding([8, 0]);
        col = col.push(self.sidebar_item("Home", Message::Home, matches!(cur, Route::Hub)));
        col = col.push(self.sidebar_item(
            "Internet Radio",
            Message::OpenCard(HubCard::Radio),
            cat_active(HubCard::Radio),
        ));
        col = col.push(
            container(text("LIBRARY").size(11).colr(carbon(p.text_muted, 1.0))).padding([12, 16]),
        );
        for (label, card) in [
            ("Albums", HubCard::Albums),
            ("Artists", HubCard::Artists),
            ("Genres", HubCard::Genres),
            ("Playlists", HubCard::Playlists),
            ("Podcasts", HubCard::Podcasts),
            ("Recently Added", HubCard::Recents),
        ] {
            col = col.push(self.sidebar_item(label, Message::OpenCard(card), cat_active(card)));
        }
        col = col.push(Space::new().height(Length::Fixed(12.0)));
        col = col.push(self.sidebar_item("Settings", Message::OpenRouting, false));
        let bg = carbon(p.background, 1.0);
        container(col)
            .height(Length::Fill)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(bg.into()),
                ..Default::default()
            })
            .into()
    }

    /// One sidebar nav row — flat (transparent idle / raised when active or
    /// hovered) with a 3px accent rail + brighter text when active.
    fn sidebar_item(&self, label: &str, msg: Message, active: bool) -> Element<'_, Message> {
        let p = mde_theme::Palette::dark();
        let raised = carbon(p.raised, 1.0);
        let accent = carbon(p.accent, 1.0);
        let text_c = if active {
            carbon(p.text, 1.0)
        } else {
            carbon(p.text_muted, 1.0)
        };
        let rail_c = if active {
            accent
        } else {
            cosmic::iced::Color::TRANSPARENT
        };
        let rail = container(
            Space::new()
                .width(Length::Fixed(3.0))
                .height(Length::Fixed(40.0)),
        )
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(rail_c.into()),
            ..Default::default()
        });
        let content = row![rail, text(label.to_string()).size(14).colr(text_c)]
            .spacing(13)
            .align_y(cosmic::iced::Alignment::Center);
        button(content)
            .width(Length::Fill)
            .padding(0)
            .on_press(msg)
            .sty(move |_t, status| {
                let bg =
                    if active || matches!(status, cosmic::iced::widget::button::Status::Hovered) {
                        Some(raised.into())
                    } else {
                        None
                    };
                cosmic::iced::widget::button::Style {
                    background: bg,
                    text_color: text_c,
                    ..cosmic::iced::widget::button::Style::default()
                }
            })
            .into()
    }

    /// MUSIC-HOME-2 — the Home page: a server-stats dashboard (hero counts +
    /// server card + count chips) fed by `action/music/library-stats`. Replaces
    /// the old 7-card hub (navigation now lives in the Carbon sidebar).
    fn home_dashboard(&self) -> Element<'_, Message> {
        let p = mde_theme::Palette::dark();
        let text_c = carbon(p.text, 1.0);
        let muted = carbon(p.text_muted, 1.0);
        let Some(s) = &self.stats else {
            return column![text("Loading library stats…").size(14).colr(muted)]
                .padding(8)
                .into();
        };
        // Hero counts (Songs / Artists / Albums).
        let stat_block = |n: u64, label: &str| -> Element<'_, Message> {
            column![
                text(commafy(n)).size(34).colr(text_c),
                text(label.to_string()).size(12).colr(muted),
            ]
            .spacing(2)
            .into()
        };
        let hero = row![
            stat_block(s.songs, "Songs"),
            stat_block(s.artists, "Artists"),
            stat_block(s.albums, "Albums"),
        ]
        .spacing(40);
        // Server card.
        let (dotc, health) = if s.reachable {
            (carbon(p.success, 1.0), "connected")
        } else {
            (carbon(p.danger, 1.0), "unreachable")
        };
        let scan = if s.scanning {
            "scanning…".to_string()
        } else {
            format!("{} songs indexed", commafy(s.songs))
        };
        let server = column![
            row![
                text("●").size(12).colr(dotc),
                Space::new().width(Length::Fixed(8.0)),
                text(format!("Airsonic · {}", s.host)).size(14).colr(text_c),
            ]
            .align_y(cosmic::iced::Alignment::Center),
            text(format!("API {} · {scan} · {health}", s.version))
                .size(12)
                .colr(muted),
        ]
        .spacing(4);
        // Count chips (Playlists / Radio / Podcasts / Genres).
        let chip = |n: u64, label: &str| -> Element<'_, Message> {
            row![
                text(commafy(n)).size(14).colr(text_c),
                Space::new().width(Length::Fixed(6.0)),
                text(label.to_string()).size(12).colr(muted),
            ]
            .align_y(cosmic::iced::Alignment::Center)
            .into()
        };
        let chips = row![
            chip(s.playlists, "Playlists"),
            chip(s.radio, "Radio"),
            chip(s.podcasts, "Podcasts"),
            chip(s.genres, "Genres"),
        ]
        .spacing(24);
        column![
            text("Your Library").size(24).colr(text_c),
            Space::new().height(Length::Fixed(18.0)),
            hero,
            Space::new().height(Length::Fixed(22.0)),
            server,
            Space::new().height(Length::Fixed(16.0)),
            chips,
        ]
        .spacing(0)
        .padding(8)
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
            Route::Hub => self.home_dashboard(),
            Route::Album(..) => self.album_page(),
            route => {
                // AIR-11.b — title + a sort toggle; items lay out in a
                // wrapping 160×160 card grid, ordered by the persisted sort.
                let title_row = row![
                    text(route.segment()).size(20),
                    Space::new().width(Length::Fill),
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
                            // MUSIC-ALBUMS-3 — Carbon album card: square art tile
                            // (raised fill + ♪ placeholder until art loads) over
                            // a 2-line title.
                            let cpal = mde_theme::Palette::dark();
                            let art_inner: Element<'_, Message> = if let Some(handle) =
                                self.art_cache.get(&item.id)
                            {
                                image(handle.clone())
                                    .width(Length::Fill)
                                    .height(Length::Fixed(150.0))
                                    .into()
                            } else {
                                container(
                                    text("\u{266A}").size(30).colr(carbon(cpal.text_muted, 1.0)),
                                )
                                .center_x(Length::Fill)
                                .center_y(Length::Fixed(150.0))
                                .into()
                            };
                            let art = container(art_inner)
                                .width(Length::Fill)
                                .height(Length::Fixed(150.0))
                                .style({
                                    let bg = carbon(cpal.raised, 1.0);
                                    move |_| cosmic::iced::widget::container::Style {
                                        background: Some(bg.into()),
                                        ..Default::default()
                                    }
                                });
                            let card_content: Element<'_, Message> = column![
                                art,
                                text(item.label.clone())
                                    .size(13)
                                    .colr(carbon(cpal.text, 1.0)),
                            ]
                            .spacing(8)
                            .into();
                            let mut btn = button(card_content)
                                .width(Length::Fixed(160.0))
                                .padding(0)
                                .sty({
                                    let accent = carbon(cpal.accent, 1.0);
                                    move |_t, status| cosmic::iced::widget::button::Style {
                                        background: None,
                                        border: cosmic::iced::Border {
                                            color: if matches!(
                                                status,
                                                cosmic::iced::widget::button::Status::Hovered
                                            ) {
                                                accent
                                            } else {
                                                cosmic::iced::Color::TRANSPARENT
                                            },
                                            width: 2.0,
                                            radius: 0.0.into(),
                                        },
                                        ..cosmic::iced::widget::button::Style::default()
                                    }
                                });
                            btn = match route {
                                Route::Category(HubCard::Albums)
                                | Route::Genre(_)
                                | Route::Artist(..)
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
                            // MUSIC-RFX-6 — on the Playlists page each card gets
                            // Rename / Delete controls beneath it (inline rename
                            // when this playlist is the active edit target).
                            if matches!(route, Route::Category(HubCard::Playlists)) {
                                let controls: Element<'_, Message> = if self
                                    .renaming_playlist
                                    .as_deref()
                                    == Some(item.id.as_str())
                                {
                                    row![
                                        text_input("name", &self.rename_buffer)
                                            .on_input(Message::RenameBufferChanged)
                                            .on_submit(Message::CommitRenamePlaylist)
                                            .width(Length::Fixed(100.0)),
                                        button(text("✓").size(12))
                                            .on_press(Message::CommitRenamePlaylist),
                                        button(text("✕").size(12))
                                            .on_press(Message::CancelRenamePlaylist),
                                    ]
                                    .spacing(4)
                                    .into()
                                } else {
                                    row![
                                        button(text("Rename").size(11)).on_press(
                                            Message::StartRenamePlaylist(
                                                item.id.clone(),
                                                item.label.clone(),
                                            ),
                                        ),
                                        button(text("Delete").size(11))
                                            .on_press(Message::DeletePlaylist(item.id.clone())),
                                    ]
                                    .spacing(4)
                                    .into()
                                };
                                r = r.push(
                                    column![btn, controls]
                                        .spacing(4)
                                        .width(Length::Fixed(160.0)),
                                );
                            } else {
                                r = r.push(btn);
                            }
                        }
                        grid = grid.push(r);
                    }
                    // MUSIC-RFX-6 — the Playlists page gets a "new playlist" form.
                    if matches!(route, Route::Category(HubCard::Playlists)) {
                        col = col.push(
                            row![
                                text_input("New playlist name…", &self.new_playlist_name)
                                    .on_input(Message::NewPlaylistNameChanged)
                                    .on_submit(Message::CreatePlaylist)
                                    .width(Length::Fixed(280.0)),
                                button(text("Create").size(13)).on_press(Message::CreatePlaylist),
                            ]
                            .spacing(8),
                        );
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
        // MUSIC-NAV — the window has no title-bar chrome, so the header carries
        // explicit Back / Home controls (left) and an Exit control (right) the
        // operator asked for, alongside the connection line + search.
        let at_root = self.nav.breadcrumb().len() <= 1;
        let nav_btn = |glyph: &str| button(text(glyph.to_string()).size(13)).padding([4, 8]);
        let back: Element<'_, Message> = if at_root {
            nav_btn("‹ Back").into()
        } else {
            nav_btn("‹ Back").on_press(Message::Back).into()
        };
        let home: Element<'_, Message> = if at_root {
            nav_btn("⌂ Home").into()
        } else {
            nav_btn("⌂ Home").on_press(Message::Home).into()
        };
        // MUSIC-ALBUMS-1 — Carbon header (48px): Back/Home + "MCNF Music"
        // wordmark, centered search, Exit. Surface background + bottom inset.
        let pal = mde_theme::Palette::dark();
        let header = container(
            row![
                back,
                home,
                text("MCNF Music").size(14).colr(carbon(pal.text, 1.0)),
                Space::new().width(Length::Fill),
                search_field,
                Space::new().width(Length::Fill),
                nav_btn("✕ Exit").on_press(Message::Exit),
            ]
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center),
        )
        .height(Length::Fixed(48.0))
        .width(Length::Fill)
        .padding([0, 16])
        .style({
            let bg = carbon(pal.background, 1.0);
            move |_| cosmic::iced::widget::container::Style {
                background: Some(bg.into()),
                ..Default::default()
            }
        });

        // Content area — breadcrumb + the body grid (the album grid etc.).
        let content = container(
            column![crumbs, Space::new().height(Length::Fixed(16.0)), body]
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill);

        // MUSIC-ALBUMS-1/2 — the Carbon grid: header / [sidebar | content] /
        // player. The persistent playback bar stays pinned at the bottom.
        let mut page_col = column![
            header,
            row![self.carbon_sidebar(), content].height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        if let Some(bar) = self.playback_bar() {
            page_col = page_col.push(bar);
        }
        let page = container(page_col).width(Length::Fill).height(Length::Fill);

        // AIR-14 — overlay the search results sheet; MUSIC-RFX-7 — overlay the
        // add-to-playlist picker (it takes priority when both could be open).
        if self.add_to_playlist_song.is_some() {
            stack![page, self.add_to_playlist_sheet()].into()
        } else if self.search_open {
            stack![page, self.search_sheet()].into()
        } else {
            page.into()
        }
    }

    /// MUSIC-RFX-7 — the add-to-playlist picker sheet: the operator's playlists
    /// as buttons (click adds the pending track via `playlist-update`), plus a
    /// Cancel. An empty roster hints to create one on the Playlists page.
    fn add_to_playlist_sheet(&self) -> Element<'_, Message> {
        let mut col = column![
            row![
                text("Add to playlist").size(18).width(Length::Fill),
                button(text("Cancel").size(13)).on_press(Message::CloseAddToPlaylist),
            ]
            .align_y(cosmic::iced::Alignment::Center),
            Space::new().height(Length::Fixed(8.0)),
        ]
        .spacing(8)
        .padding(20)
        .width(Length::Fixed(360.0));
        if self.add_to_playlist_choices.is_empty() {
            col = col.push(text("No playlists yet — create one on the Playlists page.").size(13));
        } else {
            for (id, name) in &self.add_to_playlist_choices {
                col = col.push(
                    button(text(name.clone()).size(14))
                        .width(Length::Fill)
                        .on_press(Message::AddSongToPlaylist(id.clone())),
                );
            }
        }
        container(scrollable(col))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
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
        col = col.push(Space::new().height(Length::Fixed(8.0)));
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
            Space::new().height(Length::Fixed(10.0)),
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
                // MUSIC-RFX-7 — add this track to a playlist.
                button(text("+ Playlist").size(11))
                    .on_press(Message::OpenAddToPlaylist(t.id.clone())),
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
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Color::from_rgb8(cr, cg, cb).into()), // carbon-ok: dynamic album-art colour, not a UI token
                text_color: Some(cosmic::iced::Color::from_rgb8(tr, tg, tb)), // carbon-ok: dynamic album-art colour
                ..Default::default()
            });
        let content = column![
            header_band,
            Space::new().height(Length::Fixed(16.0)),
            scrollable(list)
        ]
        .spacing(8)
        .width(Length::Fill);
        row![art, content].spacing(20).into()
    }

    /// MUSIC-PLAYBAR (2026-06-18) — the persistent playback bar, static at the
    /// bottom of every browse interface: mini cover art, title/artist, full
    /// shuttle controls (prev / play-pause / next), position, an **audio routing**
    /// control (route playback to a mesh peer — opens the Peers/take-over surface),
    /// and a Full-view toggle. `None` until a track is loaded/active.
    fn playback_bar(&self) -> Option<Element<'_, Message>> {
        if !self.now_state.has_track() && !self.now_state.active {
            return None;
        }
        let muted = carbon(mde_theme::Palette::dark().text_muted, 1.0);
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
        // Mini artwork (currently-playing cover) on its dominant-colour tint.
        let (nr, ng, nb) = self.now_color;
        let mini_inner: Element<'_, Message> = match &self.now_art {
            Some(h) => image(h.clone())
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(40.0))
                .into(),
            None => Space::new()
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(40.0))
                .into(),
        };
        let mini = container(mini_inner).style(move |_| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Color::from_rgb8(nr, ng, nb).into()), // carbon-ok: cover colour
            ..Default::default()
        });
        let meta = column![
            text(title).size(13),
            text(self.now_artist.clone()).size(11).colr(muted),
        ]
        .spacing(1)
        .width(Length::Fill);
        let pos = text(format!(
            "{}:{:02} / {}:{:02}",
            self.now_state.position_ms / 60000,
            (self.now_state.position_ms / 1000) % 60,
            self.now_duration_ms / 60000,
            (self.now_duration_ms / 1000) % 60,
        ))
        .size(11)
        .colr(muted);
        let bar = row![
            mini,
            meta,
            button(text("\u{25C0}").size(13)).on_press(Message::SkipPrev),
            button(text(play_pause).size(13)).on_press(Message::PlayPause),
            button(text("\u{25B6}").size(13)).on_press(Message::SkipNext),
            pos,
            // Audio routing — send playback to a mesh peer (AIR-8 take-over).
            button(text("\u{21C6} Route").size(12)).on_press(Message::OpenRouting),
            button(text("Full").size(12)).on_press(Message::ToggleMaxi),
        ]
        .spacing(10)
        .padding(10)
        .align_y(cosmic::iced::Alignment::Center);
        Some(container(bar).width(Length::Fill).into())
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
        // MUSIC-MAXI-SCALE (2026-06-18) — the artwork is the focus and scales
        // with the window: ~40% of the width, clamped to a sane hero range, on a
        // dominant-colour tint band (extracted from the now-playing cover). Even
        // with no art yet, the tint square holds the focal space (no collapse).
        let (nr, ng, nb) = self.now_color;
        let art_dim = (self.grid_width * 0.40).clamp(300.0, 560.0);
        let art_inner: Element<'_, Message> = match &self.now_art {
            Some(h) => image(h.clone())
                .width(Length::Fixed(art_dim))
                .height(Length::Fixed(art_dim))
                .into(),
            None => Space::new()
                .width(Length::Fixed(art_dim))
                .height(Length::Fixed(art_dim))
                .into(),
        };
        let art: Element<'_, Message> = container(art_inner)
            .padding(16)
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Color::from_rgb8(nr, ng, nb).into()), // carbon-ok: dynamic cover-art colour, not a UI token
                ..Default::default()
            })
            .into();
        // MUSIC-RFX-4 — the scrub bar. For a seekable (finite) track with a known
        // duration, render an interactive slider that jumps the playhead on drag
        // (RFX-2 `seek`); a live/radio stream (not seekable / unknown duration)
        // shows only the elapsed time — the scrubber is hidden.
        let time_label = text(format!(
            "{}:{:02} / {}:{:02}",
            self.now_state.position_ms / 60000,
            (self.now_state.position_ms / 1000) % 60,
            self.now_duration_ms / 60000,
            (self.now_duration_ms / 1000) % 60,
        ))
        .size(11);
        let scrub: Element<'_, Message> = if self.now_state.seekable && self.now_duration_ms > 0 {
            column![
                cosmic::iced::widget::slider(
                    0.0..=(self.now_duration_ms as f32),
                    self.now_state.position_ms as f32,
                    |v| Message::Seek(v as u64),
                )
                .step(1000.0_f32)
                .width(Length::Fixed(art_dim)),
                time_label,
            ]
            .spacing(2)
            .into()
        } else if self.now_duration_ms > 0 {
            let ratio = (self.now_state.position_ms as f32 / self.now_duration_ms.max(1) as f32)
                .clamp(0.0, 1.0);
            column![
                // EFF-34 — the fork renamed the bar's cross-axis setter to `girth`.
                cosmic::iced::widget::progress_bar(0.0..=1.0, ratio).girth(Length::Fixed(6.0)),
                time_label,
            ]
            .spacing(2)
            .into()
        } else {
            // Live stream: no duration to scrub against — show elapsed only.
            text(format!(
                "{}:{:02} • live",
                self.now_state.position_ms / 60000,
                (self.now_state.position_ms / 1000) % 60,
            ))
            .size(11)
            .into()
        };
        let volume_slider: Element<'_, Message> =
            cosmic::iced::widget::slider(0.0..=1.0, self.now_state.volume, Message::SetVolume)
                .step(0.01_f32)
                .width(Length::Fixed(art_dim))
                .into();
        // MUSIC-MAXI-SCALE — a full-width top bar, then a horizontally-centered
        // hero (art → title → artist → scrub → volume → transport) so the
        // artwork is the focus and the view fills the window instead of crowding
        // the top-left.
        let top_bar = row![
            text("Now Playing").size(22).width(Length::Fill),
            button(text("Close").size(13)).on_press(Message::ToggleMaxi),
        ]
        .align_y(cosmic::iced::Alignment::Center);
        let hero = container(
            column![
                art,
                text(title).size(32),
                text(self.now_artist.clone()).size(18).colr(muted),
                scrub,
                volume_slider,
                row![
                    button(text("Prev").size(13)).on_press(Message::SkipPrev),
                    button(text(play_pause).size(13)).on_press(Message::PlayPause),
                    button(text("Next").size(13)).on_press(Message::SkipNext),
                    // MUSIC-RFX-7 — add the current track to a playlist.
                    button(text("+ Playlist").size(13)).on_press_maybe(
                        (!self.now_state.song_id.is_empty())
                            .then(|| Message::OpenAddToPlaylist(self.now_state.song_id.clone())),
                    ),
                ]
                .spacing(10),
            ]
            .spacing(12)
            .align_x(cosmic::iced::Alignment::Center),
        )
        .width(Length::Fill)
        .center_x(Length::Fill);
        let header = column![top_bar, hero].spacing(12);

        // MUSIC-RFX-5 — the queue tab is now editable: per-row select / move /
        // play-next / remove, plus a "Remove selected" action. iced has no
        // native drag-drop list, so reorder is the accessible ↑/↓ affordance
        // (the same RFX-1 `queue-move` verb a drag would call).
        let last = self.queue_songs.len().saturating_sub(1);
        let header_row = row![
            text(format!("Queue ({} tracks)", self.queue_songs.len()))
                .size(15)
                .width(Length::Fill),
            button(text(format!("Remove selected ({})", self.queue_selected.len())).size(12))
                .on_press(Message::QueueRemoveSelected),
        ]
        .align_y(cosmic::iced::Alignment::Center);
        let mut queue = column![header_row].spacing(4);
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
            let selected = self.queue_selected.contains(&i);
            // GLYPH-FIX — ●/○ (text-presentation BMP), not ☑/☐: U+2611 ☑ is
            // Emoji_Presentation=Yes, so it renders via the color-emoji font
            // (ignores tint, stalls first paint). ● selected, ○ unselected.
            let sel_glyph = if selected { "\u{25CF}" } else { "\u{25CB}" };
            let mut row_el = row![
                button(text(sel_glyph).size(13)).on_press(Message::QueueToggleSelect(i)),
                text(format!("{marker}{label}"))
                    .size(13)
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(cosmic::iced::Alignment::Center);
            if i > 0 {
                row_el = row_el.push(button(text("↑").size(12)).on_press(Message::QueueMoveUp(i)));
            }
            if i < last {
                row_el =
                    row_el.push(button(text("↓").size(12)).on_press(Message::QueueMoveDown(i)));
            }
            // "Play next" is meaningless for the already-current track.
            if i != self.queue_current {
                row_el = row_el
                    .push(button(text("Play next").size(11)).on_press(Message::QueuePlayNext(i)));
            }
            row_el = row_el.push(button(text("✕").size(12)).on_press(Message::QueueRemove(i)));
            queue = queue.push(row_el);
        }

        let lyrics: Element<'_, Message> = if self.maxi_lyrics.is_empty() {
            text("No lyrics for this track").size(13).colr(muted).into()
        } else {
            let mut col = column![].spacing(2);
            for line in &self.maxi_lyrics {
                col = col.push(text(line.clone()).size(13));
            }
            col.into()
        };
        let peers: Element<'_, Message> = if self.maxi_peers.is_empty() {
            text("No peers on the mesh").size(13).colr(muted).into()
        } else {
            let mut col = column![].spacing(6);
            for p in &self.maxi_peers {
                let status = if p.playing { "● playing" } else { "paused" };
                col = col.push(
                    row![
                        text(p.host.clone()).size(14).width(Length::Fixed(150.0)),
                        text(status).size(12).colr(muted),
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
            button(text(label).size(13).colr(color)).on_press(Message::MaxiTabSelected(t))
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

/// EFF-34 — the `cosmic::Application` shell. The inherent `update`/`view`/
/// `subscription` carry the real AIR-* logic (inherent methods win direct
/// calls, so these trait methods delegate without recursion); the trait wraps
/// the reducer's iced `Task` into the cosmic `Action` space.
impl Application for State {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.mackes.MdeMusic";

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn init(core: cosmic::app::Core, _flags: ()) -> (Self, cosmic::app::Task<Self::Message>) {
        let mut s = Self::new(core);
        // Keep mde-music's in-app chrome; suppress Cosmic's headerbar so the
        // breadcrumb/connection header reads as the only top bar.
        s.core.window.show_headerbar = false;
        s.set_header_title("MDE Music".to_string());
        // MUSIC-HOME-2 — load the server stats at launch so the Home dashboard
        // is populated on first paint.
        let boot = cosmic::iced::Task::perform(
            library::fetch_library_stats(),
            Message::StatsLoaded,
        )
        .map(cosmic::Action::App);
        (s, boot)
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        State::subscription(self)
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        // Delegate to the inherent reducer (inherent resolution wins), then lift
        // the iced Task into the cosmic Action space the runtime expects.
        State::update(self, message).map(cosmic::Action::App)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        State::view(self)
    }
}

/// The stable widget id for the AIR-14 search field (so Cmd-F can focus it).
fn search_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-search")
}

/// AIR-11.c.2 — stable id for the library card grid's scrollable, so the
/// scroll position can be saved on scroll + restored on category re-entry.
fn grid_scroll_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-grid")
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
    col = col.push(Space::new().height(Length::Fixed(10.0)));
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
