//! `mde-music` binary — AIR-10/11 shell.
//!
//! Renders the 7-card library hub + a breadcrumb the user navigates,
//! plus an Airsonic connection banner (from the shared creds). The live
//! grids behind each card + playback land with the `mde-musicd` data
//! path (AIR-10.b / AIR-2); this shell is the §0.12 runtime-reachable
//! entry point that makes the [`hub`]/[`nav`] models live.
//!
//! EFF-34 — ported off iced 0.13 onto libcosmic's vendored iced. `cosmic::iced`
//! is the fork; the per-widget style closures bridge to cosmic's class-based
//! theming via [`cosmic_compat`].
//!
//! MUSIC-DOCK-1..5 — the player is a **layer-shell bottom dock**, not a normal
//! window: the shell is now a `cosmic::iced::daemon` (the `mde-notify-center` /
//! `mde-mesh-wallpaper` pattern) that maps an Overlay layer surface anchored
//! bottom+left+right, full height, with `OnDemand` keyboard and **no exclusive
//! zone** (DOCK-5 — it overlays the desktop, never reserving space). The dock
//! slides up from the bottom edge on map (DOCK-2, `set_margin`-driven, honoring
//! reduce-motion), and "closing" slides it back down to a small always-mapped
//! bottom-center handle (DOCK-3) — the process never exits on close. The
//! `view`/`update`/`subscription` reducers below carry the AIR-* logic unchanged;
//! the daemon free fns dispatch per surface and wrap the dock chrome around them.

mod cosmic_compat;
use cosmic_compat::{ButtonSty, TextSty};

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    destroy_layer_surface, get_layer_surface, set_margin, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, stack, text, text_input, Space,
};
use cosmic::iced::{window, Length, Subscription};
use cosmic::Element;

use mde_music::album::{self, AlbumView};
use mde_music::color;
use mde_music::hub::HubCard;
use mde_music::library::{self, LibraryItem};
use mde_music::nav::{NavState, Route};
use mde_music::nowplaying::{self, NowState};
use mde_music::prefs::{self, SortKey};
use mde_music::search::{self, SearchResults};
use mde_musicd::creds::{self, Creds};

/// The reducer's iced-space [`Task`]. Aliased to keep the AIR-* call sites
/// reading as plain `Task`. Under the layer-shell daemon (MUSIC-DOCK-1) the
/// runtime's message Task *is* `cosmic::iced::Task<Message>`, so the inherent
/// `update` no longer needs the cosmic-Action lift the old `Application` shell
/// required.
type Task<M> = cosmic::iced::Task<M>;

/// MUSIC-DOCK-2 — the bottom-dock slide-up: a Carbon `panel_mount` entrance
/// (`moderate-02`, 240 ms ease-out), single-sourced from `mde-theme` motion
/// tokens. The dock starts pushed [`DOCK_SLIDE_PX`] below the bottom edge (a
/// negative bottom margin) and rises to rest; under reduce-motion the tween
/// collapses to the ≤80 ms cap and the dock effectively maps in place.
const DOCK_SLIDE: mde_theme::motion::Motion = mde_theme::motion::Motion::panel_mount();
/// MUSIC-DOCK-2 — the slide-up travel distance (px): a fixed reveal offset, NOT
/// the dock's full height. A bottom-anchored full-height surface is translated by
/// a negative bottom margin; a modest fixed travel reads as a rise from the edge
/// (translating a full-height surface by its whole height would just fling it
/// entirely off-screen). Carbon's expansion tier (`moderate-02` reveals ~48px).
const DOCK_SLIDE_PX: f32 = 48.0;
/// MUSIC-DOCK-2 — the per-frame cadence for the slide tween (~60 fps). Armed
/// only while the slide is in flight (MOTION-PERF-1 — zero idle wakeups).
const SLIDE_TICK: std::time::Duration = std::time::Duration::from_millis(16);
/// MUSIC-DOCK-3 — the minimized handle's height (px): a small bottom-center tab.
const HANDLE_HEIGHT: u32 = 36;
/// MUSIC-DOCK-3 — the minimized handle's width (px).
const HANDLE_WIDTH: u32 = 280;

fn main() -> cosmic::iced::Result {
    // MUSIC-DOCK-1 — the layer-shell daemon (the notify-center / wallpaper
    // pattern): `daemon(boot, update, view)` with the title/subscription
    // builders, run to completion. The boot fn maps the dock surface.
    cosmic::iced::daemon(|| (State::new(), boot_task()), update, view)
        .title(namespace)
        .subscription(subscription)
        .theme(theme)
        .run()
}

/// MUSIC-DOCK-1 — the daemon namespace (the layer-surface namespaces are set on
/// the per-surface settings in [`boot_task`] / [`State::show_dock`]).
fn namespace(_state: &State, _id: window::Id) -> String {
    "mde-music".to_string()
}

/// MUSIC-DOCK-1 — the global theme for the dock surfaces. mde-music's whole view
/// tree builds widgets with `cosmic::Theme` (the `cosmic_compat` `.colr`/`.sty`
/// world-1 extensions), so the daemon is parameterized on `cosmic::Theme` and
/// this returns the cosmic dark theme — the player is dark-themed throughout
/// (every panel reads `mde_theme::Palette::dark()`). The per-widget Carbon
/// colours still come from the `mde-theme` tokens via [`carbon`] (§4); the
/// global theme only seeds the cosmic chrome defaults the layer surface needs.
fn theme(_state: &State, _id: window::Id) -> cosmic::Theme {
    cosmic::Theme::dark()
}

/// MUSIC-DOCK-1 — boot the dock: map the full-height bottom Overlay surface
/// (sliding up — DOCK-2) and kick the AIR-* Home load so the dock paints
/// populated on first frame.
fn boot_task() -> Task<Message> {
    Task::done(Message::ShowDock)
}

/// MUSIC-DOCK-1/3 — the daemon's free `update`: delegate to the inherent
/// reducer (which carries every AIR-* handler). No cosmic-Action lift is needed
/// under the daemon — the runtime's message Task is the inherent `Task<Message>`.
fn update(state: &mut State, message: Message) -> Task<Message> {
    state.update(message)
}

/// MUSIC-DOCK-1/3 — the daemon's free `view`: dispatch per surface. The dock
/// surface renders the full player ([`State::view`]); the minimized handle
/// surface renders the small bottom-center tab ([`State::handle_view`]).
fn view(state: &State, id: window::Id) -> Element<'_, Message> {
    if Some(id) == state.handle_surface {
        state.handle_view()
    } else {
        state.view()
    }
}

/// MUSIC-DOCK-1 — the daemon's free `subscription` delegates to the inherent one
/// (keyboard shortcuts + now-playing poll + the DOCK-2 slide tick).
fn subscription(state: &State) -> Subscription<Message> {
    State::subscription(state)
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

/// MUSIC-HOME — load everything the Home dashboard shows (stats + the
/// most-played / starred / mesh-now-playing strips) in one batch. Used on Home
/// nav, at init, and by the poll tick.
fn load_home_tasks() -> Task<Message> {
    Task::batch([
        Task::perform(library::fetch_library_stats(), Message::StatsLoaded),
        Task::perform(library::fetch("list-frequent"), |r| {
            Message::MostPlayedLoaded(r.unwrap_or_default())
        }),
        Task::perform(library::fetch("list-starred"), |r| {
            Message::StarredLoaded(r.unwrap_or_default())
        }),
        Task::perform(nowplaying::fetch_peer_states(), |r| {
            Message::HomePeersLoaded(r.unwrap_or_default())
        }),
    ])
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
/// The §4 token-derivation guard: the canonical `mde_theme::Palette::dark()`
/// mapped into an iced `Theme`, exercised by [`theme_tests`]. The live dock
/// theme is the cosmic dark theme (see [`theme`]) since the view tree is
/// `cosmic::Theme`-bound; this stays the assertion that the palette wiring is
/// intact (a future per-widget Carbon pass à la mde-files would consume it).
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
    /// MUSIC-DOCK-1 — the dock's layer surface id (the full-height bottom
    /// Overlay). `None` until the first map / while minimized to the handle.
    dock_surface: Option<window::Id>,
    /// MUSIC-DOCK-3 — the minimized-handle surface id (the small bottom-center
    /// tab). `None` while the dock is open.
    handle_surface: Option<window::Id>,
    /// MUSIC-DOCK-2 — the in-flight slide-up tween + its start instant. `None`
    /// when the dock is fully mapped at rest (no tick armed, no idle CPU).
    slide: Option<(mde_theme::animation::Tween, std::time::Instant)>,
    /// MUSIC-DOCK-2 — the desktop preference for reduced motion (resolved once
    /// at launch); the slide collapses to the ≤80 ms cap when set.
    reduce_motion: bool,
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
    /// MUSIC-HOME-3 — Home discovery strips: most-played + starred albums + the
    /// mesh now-playing roster.
    most_played: Vec<library::LibraryItem>,
    starred: Vec<library::LibraryItem>,
    home_peers: Vec<nowplaying::PeerState>,
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
    /// MUSIC-ALBUMS-4 — in-header live filter: narrows the CURRENT grid by a
    /// case-insensitive label substring (distinct from the global search sheet,
    /// which searches the whole library over the bus). Empty = no filter.
    grid_filter: String,
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
    /// MUSIC-RESPONSIVE-2 — per-route item cache (route segment → items). A
    /// re-visited Albums/Artists/etc. route paints instantly from here while a
    /// background fetch reconciles, instead of clearing + refetching (the old
    /// blank-then-flash). Bounded by the routes visited this session.
    items_cache: std::collections::HashMap<String, Vec<LibraryItem>>,
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
    /// MUSIC-RFX-6b — the open playlist editor's tracks in current order
    /// (the [`Route::Playlist`] detail page). Empty until loaded / off-route.
    playlist_tracks: Vec<library::LibraryItem>,
    /// AIR-15.b.4 — maxi tab + the current track's lyrics lines.
    maxi_tab: MaxiTab,
    maxi_lyrics: Vec<String>,
    /// AIR-15.b.5 — the mesh peer roster (Peers tab).
    maxi_peers: Vec<nowplaying::PeerState>,
    /// MUSIC-RFX-8 — the open right-click context menu (`None` = closed). Carries
    /// what was right-clicked so the sheet renders the applicable actions.
    context_menu: Option<TrackContext>,
}

/// MUSIC-RFX-8 — the target of a right-click track context menu. A track row
/// knows its song id + title; playlist removal ("Remove from playlist") is
/// offered only when the row is inside the open playlist editor.
#[derive(Debug, Clone)]
struct TrackContext {
    song_id: String,
    title: String,
    /// `Some(index)` when this row is inside the open playlist editor → "Remove".
    playlist_index: Option<usize>,
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
    /// MUSIC-HOME-4 — periodic Home-page stats refresh tick.
    PollStats,
    /// MUSIC-HOME-3 — Home discovery strips loaded.
    MostPlayedLoaded(Vec<library::LibraryItem>),
    StarredLoaded(Vec<library::LibraryItem>),
    HomePeersLoaded(Vec<nowplaying::PeerState>),
    /// MUSIC-NAV — go up one breadcrumb level (Back).
    Back,
    /// MUSIC-NAV — jump to the Library root (Home).
    Home,
    /// MUSIC-DOCK-1 — map the dock surface + start the slide-up (DOCK-2).
    ShowDock,
    /// MUSIC-DOCK-2 — advance the slide-up tween one frame.
    SlideTick,
    /// MUSIC-DOCK-3 — "close" the dock: slide it down to the bottom-center
    /// handle. The process keeps running (the dock has no titlebar chrome; this
    /// replaces the old hard `Exit`).
    Minimize,
    /// MUSIC-DOCK-3 — the minimized handle was clicked: re-map + slide the dock
    /// back up.
    RestoreDock,
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
    /// MUSIC-DOCK-1/3 — Esc: dismiss any open sheet, else minimize the dock.
    EscapePressed,
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
    /// MUSIC-RFX-6b — open the playlist reorder editor (id, name).
    OpenPlaylist(String, String),
    /// MUSIC-RFX-6b — the editor's tracks finished loading (in current order).
    PlaylistTracksLoaded(Vec<library::LibraryItem>),
    /// MUSIC-RFX-6b — move a track up / down one slot; persists the new order.
    PlaylistMoveUp(usize),
    PlaylistMoveDown(usize),
    /// MUSIC-RFX-8 — open / close the right-click track context menu.
    OpenTrackMenu(TrackContext),
    CloseContextMenu,
    /// MUSIC-RFX-8 — play this song now (clear queue → enqueue → play).
    PlaySongNow(String),
    /// MUSIC-RFX-8 — remove the track at this index from the open playlist.
    RemoveFromPlaylist(usize),
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
    /// MUSIC-ALBUMS-4 — the in-header grid filter text changed.
    GridFilterChanged(String),
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
    fn new() -> Self {
        let (form, connection) = match creds::load() {
            Ok(c) => (None, format!("Connected to {}", c.server_url)),
            Err(_) => (Some(FirstRunForm::default()), String::new()),
        };
        Self {
            // MUSIC-DOCK — surfaces are mapped by the boot ShowDock handler.
            dock_surface: None,
            handle_surface: None,
            slide: None,
            // MUSIC-DOCK-2 — resolve reduce-motion once (a11y pref or the
            // MDE_REDUCE_MOTION/MDE_MOTION_DISABLED env overrides).
            reduce_motion: mde_theme::Preferences::load().a11y.reduce_motion,
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
            most_played: Vec::new(),
            starred: Vec::new(),
            home_peers: Vec::new(),
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
            grid_filter: String::new(),
            grid_width: 1100.0,
            grid_scroll: prefs::load().scroll.into_iter().collect(),
            art_cache: std::collections::HashMap::new(),
            items_cache: std::collections::HashMap::new(),
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
            playlist_tracks: Vec::new(),
            context_menu: None,
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
    /// MUSIC-RESPONSIVE-2 — apply a freshly-loaded (or cached) item set: cache it
    /// under the current route, render it, restore the saved scroll offset, and
    /// kick the visible cover-art window. Shared by `ItemsLoaded` and the
    /// instant-paint path in `enter_route`.
    fn apply_items(&mut self, items: Vec<LibraryItem>) -> Task<Message> {
        let key = self.nav.current().segment();
        self.items_cache.insert(key.clone(), items.clone());
        self.items = items;
        self.loading = false;
        self.load_retried = false;
        let y = self.grid_scroll.get(&key).copied().unwrap_or(0.0);
        let restore = cosmic::iced::widget::scrollable::scroll_to(
            grid_scroll_id(),
            cosmic::iced::widget::scrollable::AbsoluteOffset {
                x: None,
                y: Some(y),
            },
        );
        self.art_requested.clear();
        let art = self.art_window_task(y);
        Task::batch([restore, art])
    }

    /// MUSIC-RESPONSIVE-2 — begin loading the just-pushed route. If we have its
    /// items cached, paint them instantly (a silent background fetch still runs
    /// to reconcile); otherwise clear the grid + show the loading state. Resets
    /// the per-navigation error/retry guards. Returns the instant-paint task.
    fn enter_route(&mut self) -> Task<Message> {
        self.load_error = None;
        self.load_retried = false;
        let key = self.nav.current().segment();
        if let Some(cached) = self.items_cache.get(&key).cloned() {
            self.apply_items(cached)
        } else {
            self.items.clear();
            self.loading = true;
            Task::none()
        }
    }

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
                // MUSIC-RESPONSIVE-2 — instant paint from cache (if any) + always
                // fetch in the background to reconcile.
                let seed = self.enter_route();
                // Fetch the category from the daemon over the Bus (AIR-10.b)
                // when it's backed by a verb; the rest are AIR-4.b endpoints.
                if let Some(verb) = library::verb_for(card) {
                    let fetch = Task::perform(library::fetch(verb), |r| match r {
                        Ok(items) => Message::ItemsLoaded(items),
                        Err(e) => Message::ItemsFailed(e),
                    });
                    Task::batch([seed, fetch])
                } else {
                    seed
                }
            }
            Message::ItemsLoaded(items) => {
                // MUSIC-LOCK-FIX (2026-06-18) — only the visible scroll window
                // fetches art (bounded); MUSIC-RESPONSIVE-2 caches the set so a
                // re-visit paints instantly. Both live in `apply_items`.
                self.apply_items(items)
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
                // MUSIC-RESPONSIVE-2 — stale-while-error: if the grid already shows
                // (cached) items, a background-reconcile failure must not blank it.
                // Keep the stale set; only surface the error on an empty grid.
                if !self.items.is_empty() {
                    self.loading = false;
                    return Task::none();
                }
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
                load_home_tasks()
            }
            Message::StatsLoaded(r) => {
                if let Ok(s) = r {
                    self.stats = Some(s);
                }
                Task::none()
            }
            Message::PollStats => load_home_tasks(),
            Message::MostPlayedLoaded(items) => {
                self.most_played = items;
                Task::none()
            }
            Message::StarredLoaded(items) => {
                self.starred = items;
                Task::none()
            }
            Message::HomePeersLoaded(peers) => {
                self.home_peers = peers;
                Task::none()
            }
            // MUSIC-DOCK-1/2 — map the dock surface + start the slide-up, and
            // load the Home dashboard so the dock paints populated.
            Message::ShowDock => {
                let map = self.show_dock();
                Task::batch([map, load_home_tasks()])
            }
            // MUSIC-DOCK-2 — drive the slide tween. While in flight, raise the
            // dock's bottom margin from -DOCK_SLIDE_PX toward 0 (at rest) along
            // the eased curve; when complete, clear the tween (the tick
            // subscription then disarms — no idle wakeups).
            Message::SlideTick => {
                let Some((tween, _)) = self.slide else {
                    return Task::none();
                };
                let Some(id) = self.dock_surface else {
                    self.slide = None;
                    return Task::none();
                };
                let now = std::time::Instant::now();
                let t = mde_theme::animation::ease(tween.progress(now), DOCK_SLIDE.easing);
                // SlideUp(distance): translate_y = (1 - t) * distance, i.e. the
                // dock starts DOCK_SLIDE_PX below the bottom edge and rises to 0
                // (single-sourced slide math from mde-theme's Transition).
                let offset = mde_theme::animation::Transition::SlideUp(DOCK_SLIDE_PX)
                    .params(t)
                    .translate_y;
                if tween.is_complete(now) {
                    self.slide = None;
                    set_margin(id, 0, 0, 0, 0)
                } else {
                    set_margin(id, 0, 0, -(offset.round() as i32), 0)
                }
            }
            // MUSIC-DOCK-3 — "close" slides the dock down to the handle. Destroy
            // the dock surface + map the always-present bottom-center tab. The
            // process never exits (the AIR-* poll keeps the now-playing line live
            // so the handle title stays current).
            Message::Minimize => self.minimize_to_handle(),
            // MUSIC-DOCK-3 — restore: drop the handle, re-map the dock, slide up.
            Message::RestoreDock => {
                let drop = self
                    .handle_surface
                    .take()
                    .map_or_else(Task::none, destroy_layer_surface);
                Task::batch([drop, self.show_dock()])
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
                // MUSIC-DOCK-1 — under the layer-shell daemon the message Task is
                // the iced (not cosmic-Action) Task, so focus via the iced-level
                // widget operation (the cosmic helper returns an Action Task).
                cosmic::iced::widget::operation::focus(search_id())
            }
            Message::DismissSearch => {
                self.dismiss_search();
                Task::none()
            }
            // MUSIC-DOCK-1/3 — Esc closes the topmost thing: an open sheet
            // (search / picker / context menu / maxi view) first, otherwise it
            // minimizes the dock to the handle (never exits).
            Message::EscapePressed => {
                if self.search_open
                    || self.add_to_playlist_song.is_some()
                    || self.context_menu.is_some()
                {
                    self.dismiss_search();
                    self.add_to_playlist_song = None;
                    self.add_to_playlist_choices.clear();
                    self.context_menu = None;
                    Task::none()
                } else if self.maxi_open {
                    self.maxi_open = false;
                    Task::none()
                } else {
                    self.minimize_to_handle()
                }
            }
            Message::OpenAlbum(id, name) => {
                self.context_menu = None;
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
                let seed = self.enter_route(); // MUSIC-RESPONSIVE-2 — instant on re-visit
                let fetch = Task::perform(library::fetch_albums_by_artist(id), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                });
                Task::batch([seed, fetch])
            }
            Message::OpenGenre(genre) => {
                self.nav.push(Route::Genre(genre.clone()));
                self.dismiss_search();
                let seed = self.enter_route();
                let fetch = Task::perform(library::fetch_albums_by_genre(genre), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                });
                Task::batch([seed, fetch])
            }
            Message::OpenPodcast(id, name) => {
                self.nav.push(Route::Podcast(id.clone(), name));
                self.dismiss_search();
                let seed = self.enter_route();
                let fetch = Task::perform(library::fetch_podcast_episodes(id), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                });
                Task::batch([seed, fetch])
            }
            Message::PlayEpisode(stream_id) => {
                Task::perform(album::play_ids(vec![stream_id]), Message::AlbumActionDone)
            }
            Message::PlayPlaylist(id) => {
                Task::perform(album::play_playlist(id), Message::AlbumActionDone)
            }
            // MUSIC-RFX-7 — add-to-playlist picker (reachable from any track row).
            Message::OpenAddToPlaylist(song_id) => {
                self.context_menu = None;
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
            Message::OpenPlaylist(id, name) => {
                self.nav.push(Route::Playlist(id.clone(), name));
                self.playlist_tracks.clear();
                Task::perform(album::playlist_songs(id), |r| {
                    Message::PlaylistTracksLoaded(r.unwrap_or_default())
                })
            }
            Message::PlaylistTracksLoaded(tracks) => {
                self.playlist_tracks = tracks;
                Task::none()
            }
            Message::PlaylistMoveUp(idx) => {
                if idx == 0 {
                    Task::none()
                } else {
                    self.move_playlist_track(idx, idx - 1)
                }
            }
            Message::PlaylistMoveDown(idx) => self.move_playlist_track(idx, idx + 1),
            Message::OpenTrackMenu(ctx) => {
                self.context_menu = Some(ctx);
                Task::none()
            }
            Message::CloseContextMenu => {
                self.context_menu = None;
                Task::none()
            }
            Message::PlaySongNow(id) => {
                self.context_menu = None;
                Task::perform(album::play_ids(vec![id]), Message::AlbumActionDone)
            }
            Message::RemoveFromPlaylist(idx) => {
                self.context_menu = None;
                if idx >= self.playlist_tracks.len() {
                    return Task::none();
                }
                let Route::Playlist(id, _) = self.nav.current() else {
                    return Task::none();
                };
                let id = id.clone();
                self.playlist_tracks.remove(idx);
                Task::perform(album::playlist_remove(id, idx), Message::AlbumActionDone)
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
            Message::GridFilterChanged(q) => {
                self.grid_filter = q;
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
                self.context_menu = None;
                Task::perform(album::play_next(id), Message::AlbumActionDone)
            }
            Message::AddTrackToQueue(id) => {
                self.context_menu = None;
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
                // MUSIC-RESPONSIVE-8 — optimistic: flip the play icon immediately,
                // then reconcile from the real state on TransportDone. `play_pause`
                // takes the PRE-flip state to decide the action.
                let was = self.now_state.playing;
                self.now_state.playing = !was;
                Task::perform(nowplaying::play_pause(was), |_| Message::TransportDone)
            }
            Message::SkipNext => {
                // MUSIC-RESPONSIVE-8 — a skip keeps playing; show that immediately,
                // the new track title reconciles on TransportDone.
                self.now_state.playing = true;
                Task::perform(nowplaying::skip_next(), |_| Message::TransportDone)
            }
            Message::SkipPrev => {
                self.now_state.playing = true;
                Task::perform(nowplaying::skip_prev(), |_| Message::TransportDone)
            }
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
                // Esc dismisses an open sheet, else minimizes the dock to the
                // handle (MUSIC-DOCK-1/3 — "Esc/close hides it", never exits).
                Key::Named(Named::Escape) => Some(Message::EscapePressed),
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
        // MUSIC-DOCK-2 — the slide-up frame ticker, armed ONLY while a slide is
        // in flight (MOTION-PERF-1 — a settled dock costs no idle wakeups). The
        // keyboard + resize subs run regardless so the dock stays responsive.
        let slide = if self.slide.is_some() {
            cosmic::iced::time::every(SLIDE_TICK).map(|_| Message::SlideTick)
        } else {
            Subscription::none()
        };
        // Poll the now-playing snapshot once the library is shown (there's
        // no daemon to ask on the first-run connect form).
        if self.form.is_some() {
            Subscription::batch([keys, resizes, slide])
        } else {
            let mut subs = vec![
                keys,
                resizes,
                slide,
                cosmic::iced::time::every(nowplaying::POLL).map(|_| Message::PollState),
            ];
            // MUSIC-HOME-4 — poll the server stats while the Home dashboard is
            // shown (faster while a scan is running so progress stays live).
            if matches!(self.nav.current(), Route::Hub) {
                let secs = if self.stats.as_ref().is_some_and(|s| s.scanning) {
                    5
                } else {
                    45
                };
                subs.push(
                    cosmic::iced::time::every(std::time::Duration::from_secs(secs))
                        .map(|_| Message::PollStats),
                );
            }
            Subscription::batch(subs)
        }
    }

    /// MUSIC-DOCK-1/2/5 — map the dock as a full-height bottom Overlay surface
    /// (anchored bottom+left+right) and start the slide-up. **No exclusive zone**
    /// (DOCK-5 — it overlays the desktop and never reserves space / reshapes
    /// other windows). `OnDemand` keyboard so its buttons + search field take
    /// focus on click; no titlebar (a layer surface has none). The surface is
    /// created already pushed below the bottom edge (the negative bottom margin)
    /// so the first slide tick reveals it.
    fn show_dock(&mut self) -> Task<Message> {
        // Already open → nothing to map (idempotent: a stray RestoreDock while
        // open shouldn't stack a second surface).
        if self.dock_surface.is_some() {
            return Task::none();
        }
        let id = window::Id::unique();
        self.dock_surface = Some(id);
        // MUSIC-DOCK-2 — arm the slide tween (reduce-motion → ≤80 ms cap).
        let now = std::time::Instant::now();
        let tween =
            mde_theme::animation::Tween::resolved(now, DOCK_SLIDE.duration, self.reduce_motion);
        self.slide = Some((tween, now));
        // Start DOCK_SLIDE_PX below the edge so the entrance reads (under
        // reduce-motion the tween is ~instant, so it snaps to 0 on the first
        // tick — the dock effectively maps in place, honoring reduce-motion).
        let start_bottom = -(DOCK_SLIDE_PX.round() as i32);
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            namespace: "mde-music".to_string(),
            // Full-height bottom dock: anchor all but the top is what reserves a
            // panel; anchoring bottom+left+right + no fixed height fills the
            // output vertically (the wallpaper anchors all four for full-screen;
            // the dock leaves the height free so the compositor sizes it to the
            // output, while the slide margin animates it up from the bottom).
            anchor: Anchor::BOTTOM.union(Anchor::LEFT).union(Anchor::RIGHT),
            // No fixed size → fill the anchored axes.
            size: Some((None, None)),
            // MUSIC-DOCK-5 — overlay only; reserve NO space (0 = don't push other
            // surfaces). Distinct from the notify-center, which DOES reserve.
            exclusive_zone: 0,
            layer: Layer::Overlay,
            // Interactive: buttons + the search field need clicks/focus.
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            margin: cosmic::iced::platform_specific::runtime::wayland::layer_surface::IcedMargin {
                top: 0,
                right: 0,
                bottom: start_bottom,
                left: 0,
            },
            ..Default::default()
        })
    }

    /// MUSIC-DOCK-3 — minimize: destroy the dock surface and map (if not already)
    /// the small always-present bottom-center handle. The process keeps running,
    /// so the now-playing poll continues and the handle title stays live; a click
    /// on the handle restores the dock.
    fn minimize_to_handle(&mut self) -> Task<Message> {
        self.slide = None;
        let drop = self
            .dock_surface
            .take()
            .map_or_else(Task::none, destroy_layer_surface);
        // Map the handle if it isn't already up (idempotent).
        let handle = if self.handle_surface.is_some() {
            Task::none()
        } else {
            let id = window::Id::unique();
            self.handle_surface = Some(id);
            get_layer_surface(SctkLayerSurfaceSettings {
                id,
                namespace: "mde-music-handle".to_string(),
                // Bottom-center: anchor BOTTOM only → centered horizontally.
                anchor: Anchor::BOTTOM,
                size: Some((Some(HANDLE_WIDTH), Some(HANDLE_HEIGHT))),
                // MUSIC-DOCK-5 — the handle reserves no space either.
                exclusive_zone: 0,
                layer: Layer::Overlay,
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                ..Default::default()
            })
        };
        Task::batch([drop, handle])
    }

    /// MUSIC-DOCK-3 — the minimized handle surface: a small bottom-center tab
    /// showing `♪ Music` + the now-playing title; clicking it restores the dock.
    fn handle_view(&self) -> Element<'_, Message> {
        let p = mde_theme::Palette::dark();
        let title = if !self.now_title.is_empty() {
            self.now_title.clone()
        } else if !self.now_state.song_id.is_empty() {
            self.now_state.song_id.clone()
        } else {
            "Music".to_string()
        };
        let label = row![
            text("\u{266A}").size(14).colr(carbon(p.accent, 1.0)),
            text(title).size(13).colr(carbon(p.text, 1.0)),
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center);
        let tab = button(
            container(label)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .padding([6, 14]),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .on_press(Message::RestoreDock)
        .sty({
            let raised = carbon(p.raised, 1.0);
            let overlay = carbon(p.overlay, 1.0);
            let border = carbon(p.border, 1.0);
            let text_c = carbon(p.text, 1.0);
            move |_t, status| {
                let bg = if matches!(status, cosmic::iced::widget::button::Status::Hovered) {
                    overlay
                } else {
                    raised
                };
                cosmic::iced::widget::button::Style {
                    background: Some(bg.into()),
                    text_color: text_c,
                    border: cosmic::iced::Border {
                        color: border,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..cosmic::iced::widget::button::Style::default()
                }
            }
        });
        container(tab)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
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
            HubCard::Playlists => {
                matches!(
                    cur,
                    Route::Category(HubCard::Playlists) | Route::Playlist(..)
                )
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
        // MUSIC-ALBUMS-6 — account/avatar chip pinned to the sidebar bottom: an
        // accent avatar + the live Airsonic connection line; clicking routes to
        // Settings (mesh routing prefs).
        col = col.push(Space::new().height(Length::Fill));
        let conn = if self.connection.is_empty() {
            "Not connected".to_string()
        } else {
            self.connection.clone()
        };
        let avatar = container(text("\u{25CF}").size(14).colr(carbon(p.accent, 1.0)))
            .width(Length::Fixed(28.0))
            .center_x(Length::Fixed(28.0));
        let account = button(
            row![avatar, text(conn).size(12).colr(carbon(p.text_muted, 1.0))]
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center),
        )
        .on_press(Message::OpenRouting)
        .width(Length::Fill)
        .padding([8, 16]);
        col = col.push(account);
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

        // MUSIC-HOME-3 — discovery strips: a horizontal row of clickable album
        // tiles for Most Played + Starred, then the mesh now-playing roster.
        let album_strip = |title: &str, items: &[library::LibraryItem]| -> Element<'_, Message> {
            if items.is_empty() {
                return Space::new().height(Length::Fixed(0.0)).into();
            }
            let tiles: Vec<Element<'_, Message>> = items
                .iter()
                .take(12)
                .map(|it| {
                    button(
                        text(it.label.clone())
                            .size(12)
                            .colr(text_c)
                            .width(Length::Fixed(150.0)),
                    )
                    .padding(8)
                    .on_press(Message::OpenAlbum(it.id.clone(), it.label.clone()))
                    .into()
                })
                .collect();
            column![
                text(title.to_string()).size(15).colr(text_c),
                cosmic::iced::widget::scrollable(row(tiles).spacing(8)).direction(
                    cosmic::iced::widget::scrollable::Direction::Horizontal(
                        cosmic::iced::widget::scrollable::Scrollbar::new()
                            .width(4)
                            .scroller_width(4),
                    ),
                ),
            ]
            .spacing(8)
            .into()
        };
        // Mesh now-playing: which peers are actively listening.
        let mesh_rows: Vec<Element<'_, Message>> = self
            .home_peers
            .iter()
            .map(|pr| {
                let (glyph, c) = if pr.playing {
                    ("♪", carbon(p.accent, 1.0))
                } else {
                    ("○", muted)
                };
                row![
                    text(glyph.to_string()).size(12).colr(c),
                    Space::new().width(Length::Fixed(8.0)),
                    text(pr.host.clone()).size(12).colr(text_c),
                ]
                .align_y(cosmic::iced::Alignment::Center)
                .into()
            })
            .collect();
        let mesh_section: Element<'_, Message> = if mesh_rows.is_empty() {
            Space::new().height(Length::Fixed(0.0)).into()
        } else {
            column![
                text("Now Playing across the mesh").size(15).colr(text_c),
                column(mesh_rows).spacing(4),
            ]
            .spacing(8)
            .into()
        };

        let body = column![
            text("Your Library").size(24).colr(text_c),
            Space::new().height(Length::Fixed(18.0)),
            hero,
            Space::new().height(Length::Fixed(22.0)),
            server,
            Space::new().height(Length::Fixed(16.0)),
            chips,
            Space::new().height(Length::Fixed(24.0)),
            album_strip("Most Played", &self.most_played),
            Space::new().height(Length::Fixed(20.0)),
            album_strip("Starred", &self.starred),
            Space::new().height(Length::Fixed(20.0)),
            mesh_section,
        ]
        .spacing(0)
        .padding(8);
        cosmic::iced::widget::scrollable(body).into()
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
            Route::Playlist(..) => self.playlist_page(),
            route => {
                // AIR-11.b — title + a sort toggle; items lay out in a
                // wrapping 160×160 card grid, ordered by the persisted sort.
                // MUSIC-ALBUMS-4 — header: title · in-grid filter · sort toggle.
                let title_row = row![
                    text(route.segment()).size(20),
                    Space::new().width(Length::Fill),
                    text_input("Filter…", &self.grid_filter)
                        .on_input(Message::GridFilterChanged)
                        .size(13)
                        .width(Length::Fixed(180.0)),
                    button(text(format!("Sort: {}", self.sort.label())).size(12))
                        .on_press(Message::ToggleSort),
                ]
                .spacing(8);
                let mut col = column![title_row].spacing(10);
                // AIR-11.c — width-adaptive column count (shared by the skeleton +
                // the real grid so the loading placeholder matches the layout).
                let cols = ((self.grid_width + 8.0) / 168.0).floor().max(1.0) as usize;
                if self.loading {
                    // MUSIC-RESPONSIVE-6 — greyed Carbon skeleton tiles (matching
                    // the card geometry) instead of a blank "Loading…" line, so a
                    // navigation paints structure within one frame.
                    col = col.push(skeleton_grid(cols));
                } else if let Some(err) = &self.load_error {
                    col = col.push(text(err.clone()).size(13));
                } else if self.items.is_empty() {
                    col = col.push(
                        text("Nothing here yet — start mde-musicd to load your library.").size(13),
                    );
                } else {
                    let mut items = self.items.clone();
                    // MUSIC-ALBUMS-4 — apply the in-header filter (case-insensitive
                    // label substring) before sort + layout.
                    let needle = self.grid_filter.trim().to_lowercase();
                    if !needle.is_empty() {
                        items.retain(|it| it.label.to_lowercase().contains(&needle));
                        if items.is_empty() {
                            col = col.push(
                                text(format!("No matches for \u{201c}{}\u{201d}.", needle))
                                    .size(13),
                            );
                        }
                    }
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
                    // MUSIC-RESPONSIVE-9 — virtualize large grids: render only the
                    // visible row window (+overscan) and reserve the off-window
                    // height with spacers, so a multi-hundred-card library doesn't
                    // build every card per frame. Skipped for the Playlists page
                    // (variable-height cards + inline forms) and small grids (where
                    // full render is cheaper than the windowing bookkeeping). Same
                    // ROW_PITCH/window as `art_window_task`, and the live scroll
                    // offset comes from `grid_scroll` (kept current by GridScrolled).
                    const ROW_PITCH: f32 = 168.0;
                    const WINDOW_ROWS: usize = 14;
                    let total_rows = items.len().div_ceil(cols);
                    let virtualize = !matches!(route, Route::Category(HubCard::Playlists))
                        && total_rows > WINDOW_ROWS;
                    let offset_y = self
                        .grid_scroll
                        .get(&route.segment())
                        .copied()
                        .unwrap_or(0.0);
                    let start_row = if virtualize {
                        ((offset_y / ROW_PITCH).floor() as usize).saturating_sub(2)
                    } else {
                        0
                    };
                    let end_row = if virtualize {
                        (start_row + WINDOW_ROWS).min(total_rows)
                    } else {
                        total_rows
                    };
                    let mut grid = column![].spacing(8);
                    if virtualize && start_row > 0 {
                        grid = grid
                            .push(Space::new().height(Length::Fixed(start_row as f32 * ROW_PITCH)));
                    }
                    for (row_idx, chunk) in items.chunks(cols).enumerate() {
                        if virtualize && (row_idx < start_row || row_idx >= end_row) {
                            continue;
                        }
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
                                    // MUSIC-ALBUMS-7 — card hover outline reads
                                    // the Carbon Blue-50 accent token (#4589ff).
                                    let accent = carbon(mde_theme::carbon::BLUE_50, 1.0);
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
                                        // MUSIC-RFX-6b — open the reorder editor.
                                        button(text("Edit").size(11)).on_press(
                                            Message::OpenPlaylist(
                                                item.id.clone(),
                                                item.label.clone(),
                                            ),
                                        ),
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
                    if virtualize && end_row < total_rows {
                        grid = grid.push(
                            Space::new()
                                .height(Length::Fixed((total_rows - end_row) as f32 * ROW_PITCH)),
                        );
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
        // wordmark, centered search, and the MUSIC-DOCK-3 minimize control.
        // Surface background + bottom inset.
        let pal = mde_theme::Palette::dark();
        let header = container(
            row![
                back,
                home,
                text("MCNF Music").size(14).colr(carbon(pal.text, 1.0)),
                Space::new().width(Length::Fill),
                search_field,
                Space::new().width(Length::Fill),
                // MUSIC-DOCK-3 — "close" minimizes the dock to the bottom-center
                // handle (the process keeps running); it never exits.
                nav_btn("⌄ Minimize").on_press(Message::Minimize),
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
        } else if self.context_menu.is_some() {
            // MUSIC-RFX-8 — right-click menu (add-to-playlist, opened from it,
            // takes priority since it clears the menu).
            stack![page, self.context_menu_sheet()].into()
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

    /// MUSIC-RFX-8 — the right-click track context menu: an action list over a
    /// full-screen dismiss backdrop. Actions reuse the existing track messages;
    /// "Remove from playlist" appears only when the row is in the playlist
    /// editor. iced has no native cursor-anchored popup, so the menu centers
    /// (a cursor-anchored position is a polish follow-on).
    fn context_menu_sheet(&self) -> Element<'_, Message> {
        let Some(ctx) = &self.context_menu else {
            return Space::new().into();
        };
        let item = |label: &str, msg: Message| {
            button(text(label.to_string()).size(13))
                .width(Length::Fill)
                .on_press(msg)
        };
        let mut col = column![
            text(ctx.title.clone()).size(14),
            Space::new().height(Length::Fixed(6.0)),
            item("Play", Message::PlaySongNow(ctx.song_id.clone())),
            item("Play next", Message::PlayTrackNext(ctx.song_id.clone())),
            item(
                "Add to queue",
                Message::AddTrackToQueue(ctx.song_id.clone())
            ),
            item(
                "Add to playlist…",
                Message::OpenAddToPlaylist(ctx.song_id.clone())
            ),
        ]
        .spacing(2)
        .padding(12)
        .width(Length::Fixed(240.0));
        if let Some(idx) = ctx.playlist_index {
            col = col.push(item(
                "Remove from playlist",
                Message::RemoveFromPlaylist(idx),
            ));
        }
        col = col.push(Space::new().height(Length::Fixed(6.0)));
        col = col.push(item("Cancel", Message::CloseContextMenu));

        // Full-screen dismiss backdrop beneath the centered menu box.
        let backdrop = mouse_area(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::CloseContextMenu);
        let menu = container(col).center_x(Length::Fill).center_y(Length::Fill);
        stack![backdrop, menu].into()
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
    /// MUSIC-RFX-6b — move the playlist track at `from` to `to` (an adjacent
    /// swap), then persist the whole new order to the daemon. Out-of-range or
    /// no-op moves do nothing.
    fn move_playlist_track(&mut self, from: usize, to: usize) -> Task<Message> {
        let n = self.playlist_tracks.len();
        if from >= n || to >= n || from == to {
            return Task::none();
        }
        self.playlist_tracks.swap(from, to);
        let Route::Playlist(id, _) = self.nav.current() else {
            return Task::none();
        };
        let id = id.clone();
        let order: Vec<String> = self.playlist_tracks.iter().map(|t| t.id.clone()).collect();
        Task::perform(album::playlist_reorder(id, order), Message::AlbumActionDone)
    }

    /// MUSIC-RFX-6b — the playlist reorder editor: the tracks in order, each
    /// with ↑/↓ controls (the accessible reorder affordance — iced has no
    /// native drag-drop list, mirroring the RFX-5 queue rows). A reorder
    /// persists in place via `playlist-reorder`, preserving the playlist id.
    fn playlist_page(&self) -> Element<'_, Message> {
        let Route::Playlist(id, name) = self.nav.current() else {
            return text("No playlist.").size(13).into();
        };
        let header = column![
            text(name.clone()).size(24),
            text(format!(
                "{} track(s) · reorder with ↑/↓",
                self.playlist_tracks.len()
            ))
            .size(12),
            Space::new().height(Length::Fixed(10.0)),
            button(text("Play all")).on_press(Message::PlayPlaylist(id.clone())),
        ]
        .spacing(4);

        let mut list = column![].spacing(4);
        if self.playlist_tracks.is_empty() {
            list = list.push(text("This playlist is empty.").size(13));
        }
        let last = self.playlist_tracks.len().saturating_sub(1);
        for (i, t) in self.playlist_tracks.iter().enumerate() {
            let mut row_el = row![
                text(format!("{}.", i + 1))
                    .size(13)
                    .width(Length::Fixed(32.0)),
                text(t.label.clone()).size(13).width(Length::Fill),
            ]
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);
            if i > 0 {
                row_el =
                    row_el.push(button(text("↑").size(12)).on_press(Message::PlaylistMoveUp(i)));
            }
            if i < last {
                row_el =
                    row_el.push(button(text("↓").size(12)).on_press(Message::PlaylistMoveDown(i)));
            }
            // MUSIC-RFX-8 — right-click for the action menu (incl. Remove).
            let menu_ctx = TrackContext {
                song_id: t.id.clone(),
                title: t.label.clone(),
                playlist_index: Some(i),
            };
            list = list.push(mouse_area(row_el).on_right_press(Message::OpenTrackMenu(menu_ctx)));
        }

        column![header, Space::new().height(Length::Fixed(12.0)), list]
            .spacing(6)
            .padding(8)
            .into()
    }

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
            // MUSIC-RFX-8 — right-click the row for the dense action menu.
            let menu_ctx = TrackContext {
                song_id: t.id.clone(),
                title: t.title.clone(),
                playlist_index: None,
            };
            list =
                list.push(mouse_area(track_row).on_right_press(Message::OpenTrackMenu(menu_ctx)));
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

/// The stable widget id for the AIR-14 search field (so Cmd-F can focus it).
fn search_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-search")
}

/// AIR-11.c.2 — stable id for the library card grid's scrollable, so the
/// scroll position can be saved on scroll + restored on category re-entry.
fn grid_scroll_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-grid")
}

/// MUSIC-RESPONSIVE-6 — a grid of greyed Carbon skeleton tiles shown while a
/// category loads, matching the real card geometry (160px col, 150px art tile +
/// a short label bar) so navigation paints structure within one frame instead of
/// a blank pane. Static (no shimmer — that's the MOTION epic); `cols` mirrors the
/// real grid's width-adaptive column count.
fn skeleton_grid(cols: usize) -> Element<'static, Message> {
    let cpal = mde_theme::Palette::dark();
    let fill = carbon(cpal.raised, 1.0);
    let block = move |w: Length, h: f32| -> Element<'static, Message> {
        container(Space::new().width(w).height(Length::Fixed(h)))
            .style(move |_| cosmic::iced::widget::container::Style {
                background: Some(fill.into()),
                ..Default::default()
            })
            .into()
    };
    let tile = move || -> Element<'static, Message> {
        column![
            block(Length::Fill, 150.0),
            block(Length::Fixed(110.0), 12.0),
        ]
        .spacing(8)
        .width(Length::Fixed(160.0))
        .into()
    };
    // Two rows of placeholders — enough to fill the typical viewport.
    let mut grid = column![].spacing(8);
    for _ in 0..2 {
        let mut r = row![].spacing(8);
        for _ in 0..cols {
            r = r.push(tile());
        }
        grid = grid.push(r);
    }
    grid.into()
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
