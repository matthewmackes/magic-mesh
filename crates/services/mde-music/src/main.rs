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
//! `mde-mesh-wallpaper` pattern) that maps a full-height Overlay layer surface
//! anchored on all four edges (top+bottom+left+right, the wallpaper pattern —
//! MUSIC-DOCK-NORENDER: anchoring only three edges leaves the vertical axis
//! unspanned, so the compositor maps it at zero height and nothing renders),
//! with `OnDemand` keyboard and **no exclusive zone** (DOCK-5 — it overlays the
//! desktop, never reserving space). The dock
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
use mde_music::density::{GridMetrics, ListMetrics};
use mde_music::hub::HubCard;
use mde_music::library::{self, LibraryItem};
use mde_music::motion;
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
/// tokens. The dock's content starts pushed [`DOCK_SLIDE_PX`] down (a positive
/// top margin on the four-edge-anchored surface) and rises to rest; under
/// reduce-motion the tween collapses to the ≤80 ms cap and the dock effectively
/// maps in place.
const DOCK_SLIDE: mde_theme::motion::Motion = mde_theme::motion::Motion::panel_mount();
/// MUSIC-DOCK-2 — the slide-up travel distance (px): a fixed reveal offset, NOT
/// the dock's full height. The full-height (four-edge-anchored) surface's top
/// margin is tweened from this offset to 0, so the content rises into place; a
/// modest fixed travel reads as a rise (tweening by the whole height would just
/// fling it off-screen). Carbon's expansion tier (`moderate-02` reveals ~48px).
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

/// BEAUT-MUSIC — the single "is shell motion suppressed?" predicate, resolved
/// once at launch. `true` when the user asked for reduced motion (a11y pref /
/// `MDE_REDUCE_MOTION`) **or** the MOTION-CORE-3 master kill switch is off
/// (`motion.enabled == false` / `MDE_MOTION_DISABLED`). Both collapse every
/// shell animation to the static/final frame, so the welcome reveal, skeleton
/// shimmer, hover-lift, and slide-up all gate on this one value — there is never
/// a path where the kill switch is set but a surface still animates.
fn motion_suppressed() -> bool {
    let prefs = mde_theme::Preferences::load();
    prefs.a11y.reduce_motion || !prefs.motion.enabled
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

/// MEDIA-8 — what the player should show on launch, decided PURELY from whether
/// creds exist (so the auto-browse-vs-first-run rule is unit-tested apart from
/// the cosmic runtime + the filesystem).
///
/// The whole point of MEDIA-8's auto-config: a fresh Workstation has
/// `airsonic-creds.json` already written (by `mackesd`'s `music_autoconfig`
/// worker against the mesh shared account), so the player BROWSES the library
/// on launch. The first-run connect form is now an OVERRIDE — reachable via the
/// manual "change server" action ([`Message::ChangeServer`]) or surfaced on an
/// auth failure — NOT the default gate it used to be. Only a genuinely
/// unconfigured node (no creds at all) lands on first-run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchView {
    /// Creds exist → auto-browse the library (no manual connect).
    Browse,
    /// No creds → show the first-run connect form (the unconfigured fallback).
    FirstRun,
}

impl LaunchView {
    /// Decide the launch view from creds-presence. `true` = creds loaded OK.
    fn from_creds_present(present: bool) -> Self {
        if present {
            Self::Browse
        } else {
            Self::FirstRun
        }
    }
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
    /// MOTION-NET-4 — the in-flight optimistic transport action + the `now_state`
    /// captured BEFORE it was applied. `Some` while a transport RPC is pending; on
    /// failure the snapshot reverts the optimistic flip so the footer keeps its
    /// prior context instead of blanking. Cleared on success/failure resolution.
    pending_transport: Option<(TransportAction, NowState)>,
    /// MOTION-NET-4 — the last transport action that failed, surfaced as a
    /// non-blocking "… — Retry" banner in the footer. `None` when the last action
    /// succeeded (or none has run). Retrying re-issues exactly this action.
    failed_transport: Option<TransportAction>,
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
    /// MUSIC-RFX-9 — the maxi Queue list's live scroll offset (y), kept current by
    /// [`Message::QueueScrolled`]. Drives the row-window virtualization in
    /// [`Self::maxi_view`] so a multi-thousand-track queue renders only the visible
    /// rows (the same spacer-windowing pattern the library grid uses, RESPONSIVE-9).
    queue_scroll: f32,
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
    /// POLISH-music-errorretry — a playlist-tracks fetch is in flight. While set,
    /// the playlist page shows the breathing skeleton instead of falsely claiming
    /// "This playlist is empty." — the empty line is honest only once the load has
    /// actually completed (§7: loading is distinct from empty).
    playlist_loading: bool,
    /// AIR-15.b.4 — maxi tab + the current track's lyrics lines.
    maxi_tab: MaxiTab,
    maxi_lyrics: Vec<String>,
    /// AIR-15.b.5 — the mesh peer roster (Peers tab).
    maxi_peers: Vec<nowplaying::PeerState>,
    /// MUSIC-RFX-8 — the open right-click context menu (`None` = closed). Carries
    /// what was right-clicked so the sheet renders the applicable actions.
    context_menu: Option<TrackContext>,
    /// MOTION-FEEDBACK — the now-playing footer's gentle reveal tween (a fresh
    /// track fades-and-rises in). `None` once settled (no reveal tick armed).
    now_reveal: Option<motion::Reveal>,
    /// MOTION-FEEDBACK — the maxi Queue list's staggered reveal tween, restarted
    /// on each (re)load so inserted/refreshed rows reveal top-down. `None` once
    /// settled.
    queue_reveal: Option<motion::Reveal>,
    /// BEAUT-MUSIC — the first-paint mount reveal for the welcome card (first
    /// open) and the Home dashboard (once stats land): a single fade-and-rise so
    /// the surface settles in instead of snapping. `None` once settled (no mount
    /// tick armed). Started at launch; restarted when the Home dashboard's
    /// skeleton is first replaced by real content (staged reveal).
    mount: Option<motion::MountReveal>,
    /// BEAUT-MUSIC — the breathing skeleton placeholder fill, used while the
    /// library grid / Home dashboard is still loading the daemon state. The shared
    /// Carbon primitive ([`mde_theme::SkeletonShimmer`]) — its breathe, fill, and
    /// reduce-motion contract are single-sourced in `mde-theme`, not re-derived
    /// here. Static (no tick) under reduce-motion / the motion kill switch; the
    /// subscription gates on [`SkeletonShimmer::needs_tick`] (fed
    /// [`State::skeleton_visible`]) so it costs nothing once content lands.
    shimmer: mde_theme::SkeletonShimmer,
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

/// MOTION-NET-4 — a transport command the GUI applied optimistically (the play
/// icon flipped / playhead jumped before the daemon confirmed). The variant
/// carries everything needed to **re-issue** it on a retry; the pre-action
/// `NowState` snapshot for the **revert** is held separately in
/// [`State::pending_transport`] so a failed action restores the exact prior view
/// instead of blanking the footer. Cheap to clone (POD), so it rides the in-flight
/// task and the retry message.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TransportAction {
    /// Toggle play/pause; `was_playing` is the PRE-flip state `play_pause` reads
    /// to pick pause-vs-resume (so a retry issues the same verb).
    PlayPause { was_playing: bool },
    /// Skip to the next queued track.
    SkipNext,
    /// Skip to the previous queued track.
    SkipPrev,
    /// Set the volume to `level` (0.0..=1.0).
    SetVolume { level: f32 },
    /// Seek the current track to `position_ms`.
    Seek { position_ms: u64 },
}

/// MOTION-NET-4 — what the update loop should do when a transport RPC resolves,
/// computed purely so the reconcile-vs-revert decision is unit-testable apart from
/// the cosmic runtime. The handler applies the resulting [`State`] mutations + the
/// follow-up task.
#[derive(Debug, Clone, PartialEq)]
enum TransportOutcome {
    /// The action succeeded: clear the pending/failed state and reconcile from a
    /// fresh `get-state` fetch (the daemon is the truth).
    Reconcile,
    /// The action failed: revert `now_state` to this pre-action snapshot and arm
    /// the retry banner for `action`.
    Revert {
        snapshot: NowState,
        action: TransportAction,
    },
    /// A stale completion for an action that's no longer the pending one (a newer
    /// action superseded it). Ignore it: the newer action owns the optimistic
    /// state + the pending slot, so neither revert nor disturb the pending slot.
    /// (A stale success doesn't need a reconcile either — the newer action's own
    /// completion will reconcile.)
    Ignore,
}

/// MOTION-NET-4 — pure decision for a resolved transport RPC. `pending` is the
/// in-flight `(action, pre-action snapshot)`; `result` is the RPC outcome. A
/// completion whose action no longer matches the pending slot is stale (a newer
/// action raced ahead) and is ignored — the newer action owns the state, so we
/// never revert to an outdated snapshot or blank the footer.
fn resolve_transport_outcome(
    pending: Option<(TransportAction, NowState)>,
    action: TransportAction,
    result: Result<(), ()>,
) -> TransportOutcome {
    let snapshot = match pending {
        Some((a, snap)) if a == action => snap,
        // Not the current pending action → stale completion, ignore.
        _ => return TransportOutcome::Ignore,
    };
    match result {
        Ok(()) => TransportOutcome::Reconcile,
        Err(()) => TransportOutcome::Revert { snapshot, action },
    }
}

impl TransportAction {
    /// The async Bus call that performs this action, as a [`Task`] resolving to
    /// a [`Message::TransportDone`] carrying the action + its outcome (so the
    /// update loop can reconcile on success or revert + offer a retry on failure).
    fn perform(self) -> Task<Message> {
        let fut = async move {
            match self {
                Self::PlayPause { was_playing } => nowplaying::play_pause(was_playing).await,
                Self::SkipNext => nowplaying::skip_next().await,
                Self::SkipPrev => nowplaying::skip_prev().await,
                Self::SetVolume { level } => nowplaying::set_volume(level).await,
                Self::Seek { position_ms } => nowplaying::seek(position_ms).await,
            }
        };
        Task::perform(fut, move |r| {
            Message::TransportDone(self, r.map_err(|_| ()))
        })
    }

    /// A short, user-facing description of what failed, for the retry banner.
    fn failed_label(self) -> &'static str {
        match self {
            Self::PlayPause { .. } => "Couldn't change playback",
            Self::SkipNext => "Couldn't skip to the next track",
            Self::SkipPrev => "Couldn't skip to the previous track",
            Self::SetVolume { .. } => "Couldn't set the volume",
            Self::Seek { .. } => "Couldn't seek",
        }
    }
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
    /// MUSIC-RFX-9 — the maxi Queue list scrolled; record the offset so the row
    /// window (virtualization) follows the viewport.
    QueueScrolled(f32),
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
    /// MEDIA-8 — open the connect form as an OVERRIDE even when creds already
    /// exist ("change server"), pre-filled from the current creds. This is the
    /// manual escape hatch now that auto-config browses by default; also used to
    /// re-surface the form on an auth failure against the shared account.
    ChangeServer,
    /// MEDIA-8 — dismiss the change-server override form without saving, return
    /// to browsing (only meaningful when creds already exist).
    CancelChangeServer,
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
    /// A transport command (volume/play/skip/seek) finished. MOTION-NET-4 — the
    /// outcome now drives reconcile-vs-revert: `Ok` reconciles from a fresh state
    /// fetch (the truth); `Err` reverts the optimistic flip to the pre-action
    /// snapshot and surfaces a retry banner, never losing context.
    TransportDone(TransportAction, Result<(), ()>),
    /// MOTION-NET-4 — re-issue the last failed transport action (the retry banner
    /// button), re-applying it optimistically against the current state.
    RetryTransport,
    /// MOTION-FEEDBACK — advance the in-flight now-playing / queue reveal one
    /// frame. Armed only while a reveal is animating (MOTION-PERF-1).
    RevealTick,
    /// BEAUT-MUSIC — advance the first-paint mount reveal one frame; clears it
    /// once settled so the tick disarms. Armed only while the mount is in flight.
    MountTick,
    /// BEAUT-MUSIC — repaint a breathing skeleton placeholder one frame. Armed
    /// only while a skeleton is on screen AND motion is on (MOTION-PERF-1).
    ShimmerTick,
}

impl State {
    fn new() -> Self {
        // MEDIA-8 — auto-browse when creds exist (the fresh-node birthright: the
        // mackesd `music_autoconfig` worker has already written
        // `airsonic-creds.json` against the mesh shared account), else land on
        // the first-run form. The decision is the pure `LaunchView`; the form is
        // an OVERRIDE reachable later via `Message::ChangeServer`, not the only
        // gate.
        let loaded = creds::load();
        let (form, connection) = match LaunchView::from_creds_present(loaded.is_ok()) {
            LaunchView::Browse => {
                let c = loaded.expect("Browse only when creds loaded");
                (None, format!("Connected to {}", c.server_url))
            }
            LaunchView::FirstRun => (Some(FirstRunForm::default()), String::new()),
        };
        // BEAUT-MUSIC — resolve the effective reduce-motion (a11y pref OR the
        // MOTION-CORE-3 kill switch) once; the welcome/skeleton/feedback all gate
        // on it. The launch instant seeds the first-paint mount + the shimmer.
        let reduce_motion = motion_suppressed();
        let now = std::time::Instant::now();
        Self {
            // MUSIC-DOCK — surfaces are mapped by the boot ShowDock handler.
            dock_surface: None,
            handle_surface: None,
            slide: None,
            // MUSIC-DOCK-2 / BEAUT-MUSIC — the effective reduce-motion resolved
            // above (a11y pref OR the MOTION-CORE-3 master kill switch). The kill
            // switch collapses motion exactly like reduce-motion (Q32 contract),
            // so one flag gates every animation.
            reduce_motion,
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
            pending_transport: None,
            failed_transport: None,
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
            queue_scroll: 0.0,
            add_to_playlist_song: None,
            add_to_playlist_choices: Vec::new(),
            new_playlist_name: String::new(),
            renaming_playlist: None,
            playlist_tracks: Vec::new(),
            playlist_loading: false,
            context_menu: None,
            rename_buffer: String::new(),
            maxi_tab: MaxiTab::Queue,
            maxi_lyrics: Vec::new(),
            maxi_peers: Vec::new(),
            now_reveal: None,
            queue_reveal: None,
            // BEAUT-MUSIC — the surface gently reveals on first open (the welcome
            // card or, once creds exist, the dock chrome settling in over its
            // skeleton). Restarted when the Home dashboard's stats land.
            mount: Some(motion::MountReveal::starting_at(now, reduce_motion)),
            // BEAUT-MUSIC — the effective reduce-motion is already folded once at
            // launch (`motion_suppressed`: a11y pref OR the kill switch), so seed
            // the shared shimmer with that resolved flag rather than re-reading
            // prefs.
            shimmer: mde_theme::SkeletonShimmer::new(now, reduce_motion),
        }
    }

    /// MUSIC-RFX-9 — snap the (re)mounted maxi Queue scrollable to the saved
    /// `queue_scroll` offset, so the virtualization window lines up with the
    /// viewport. The Queue scrollable only exists in the widget tree while the
    /// Queue tab is showing, so every path that (re)mounts it — reopening the maxi,
    /// reloading after a reorder/remove (`QueueLoaded`), switching back from the
    /// Lyrics/Peers tab — must restore the offset, mirroring how `apply_items`
    /// restores the grid's per-route scroll.
    fn restore_queue_scroll(&self) -> Task<Message> {
        cosmic::iced::widget::scrollable::scroll_to(
            queue_scroll_id(),
            cosmic::iced::widget::scrollable::AbsoluteOffset {
                x: None,
                y: Some(self.queue_scroll),
            },
        )
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
            // POLISH-music-errorretry — the album detail page re-fetches its own
            // view (not a grid of LibraryItems), so the shared Retry affordance on
            // an album load failure is live rather than a no-op.
            Route::Album(id, _) => Task::perform(album::fetch_album(id.clone()), |r| match r {
                Ok(a) => Message::AlbumLoaded(a),
                Err(e) => Message::AlbumFailed(e),
            }),
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
                // POLISH-music-errorretry — the one retry entry point for every
                // browse surface. Reset only the *current* surface's load flags
                // (so the right error clears and the right skeleton arms), then
                // re-dispatch via `reload_current`. The album page tracks its own
                // `album_*` flags; the grid/artist/genre/podcast routes share the
                // `load_*` pair.
                if matches!(self.nav.current(), Route::Album(..)) {
                    self.album = None;
                    self.album_error = None;
                    self.album_loading = true;
                } else {
                    self.load_error = None;
                    self.loading = true;
                }
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
                // MOTION-FEEDBACK — a (re)loaded queue reveals its rows top-down
                // (staggered slide-in), so a refresh isn't abrupt. Only when the
                // list is non-empty (an empty reveal would just arm a no-op tick).
                self.queue_reveal = (!songs.is_empty()).then(|| {
                    motion::Reveal::starting_at(std::time::Instant::now(), self.reduce_motion)
                });
                self.queue_songs = songs;
                let mut tasks: Vec<Task<Message>> = self
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
                // MUSIC-RFX-9 — keep the (re)mounted Queue scrollable's real offset
                // in sync with `queue_scroll`, the same way the grid restores its
                // saved offset in `apply_items`. Without this, reopening the maxi (or
                // any reload after a reorder/remove) mounts the scrollable at the top
                // while `queue_scroll` still holds a deep offset → the virtualization
                // window would mount off-screen and the user would see only the
                // leading spacer (a blank Queue) until the next scroll re-synced it.
                tasks.push(self.restore_queue_scroll());
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
                    // MUSIC-RFX-9 — the Queue scrollable is remounted (it only
                    // exists while its tab is active), so restore its offset or the
                    // virtualization window would mount off-screen (blank Queue).
                    MaxiTab::Queue => self.restore_queue_scroll(),
                    MaxiTab::Lyrics => Task::none(),
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
                    // BEAUT-MUSIC — staged reveal: the very first stats batch
                    // replaces the Home skeleton with real content, so fade-and-
                    // rise the dashboard in (a fresh mount epoch). Later refresh
                    // ticks just update the numbers in place (no re-reveal).
                    if self.stats.is_none() {
                        self.mount = Some(motion::MountReveal::starting_at(
                            std::time::Instant::now(),
                            self.reduce_motion,
                        ));
                    }
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
            // MUSIC-DOCK-2 — drive the slide tween. While in flight, shrink the
            // dock's TOP margin from +DOCK_SLIDE_PX toward 0 (at rest) along the
            // eased curve; when complete, clear the tween (the tick subscription
            // then disarms — no idle wakeups).
            //
            // MUSIC-DOCK-NORENDER — the surface is now anchored on all four edges
            // (full height), so a margin offsets only ITS OWN edge: a positive
            // top margin pushes the whole dock's top edge DOWN by that many px
            // (the content, which lays out top-down, starts lower and rises into
            // place). A negative *bottom* margin — what this used to drive — would
            // only extend the bottom edge below the screen while the top (and all
            // the visible content) stayed pinned, so it produced no visible rise.
            // The reveal therefore animates the top margin instead.
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
                // dock starts DOCK_SLIDE_PX down (top margin = +offset) and rises
                // to 0 (single-sourced slide math from mde-theme's Transition).
                let offset = mde_theme::animation::Transition::SlideUp(DOCK_SLIDE_PX)
                    .params(t)
                    .translate_y;
                if tween.is_complete(now) {
                    self.slide = None;
                    set_margin(id, 0, 0, 0, 0)
                } else {
                    set_margin(id, offset.round() as i32, 0, 0, 0)
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
            // MEDIA-8 — "change server" override: open the connect form even
            // though creds exist, pre-filled from the saved creds so the user
            // edits rather than retypes. The first-run form is no longer the
            // default gate (auto-config browses); this is the manual escape hatch.
            Message::ChangeServer => {
                let cur = creds::load().ok();
                self.form = Some(FirstRunForm {
                    url: cur
                        .as_ref()
                        .map(|c| c.server_url.clone())
                        .unwrap_or_default(),
                    user: cur.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
                    pass: cur.map(|c| c.password).unwrap_or_default(),
                    error: None,
                });
                Task::none()
            }
            // MEDIA-8 — back out of the override form without saving. Only
            // returns to browsing when creds still exist (otherwise the node is
            // genuinely unconfigured and the first-run form must stay).
            Message::CancelChangeServer => {
                if creds::load().is_ok() {
                    self.form = None;
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
                // POLISH-music-errorretry — starting a search clears any prior
                // failure so the in-flight skeleton shows (and a later success is
                // never shadowed by the stale error). This also makes the search
                // error block's Retry → re-run-this-query path correct.
                self.search_error = None;
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
                // POLISH-music-errorretry — mark the fetch in flight so the page
                // shows a loading skeleton, not the "empty" line, until it lands.
                self.playlist_loading = true;
                Task::perform(album::playlist_songs(id), |r| {
                    Message::PlaylistTracksLoaded(r.unwrap_or_default())
                })
            }
            Message::PlaylistTracksLoaded(tracks) => {
                self.playlist_tracks = tracks;
                // The load settled (with tracks, or genuinely empty) — drop the
                // skeleton so the real list / honest empty state shows.
                self.playlist_loading = false;
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
            Message::QueueScrolled(y) => {
                // MUSIC-RFX-9 — track the maxi Queue scroll so the row window
                // (virtualization) re-centers on the viewport. No I/O — the queue
                // titles are already resolved on load (QueueLoaded), so this only
                // advances which rows the next frame builds.
                self.queue_scroll = y;
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
                // MOTION-NET-4 — a transport RPC is in flight: its optimistic state
                // is authoritative until it resolves (Reconcile re-fetches, or Revert
                // restores the snapshot). Don't let a racing 2s background poll clobber
                // the optimistic flip — that would un-flip the play icon for a frame, or
                // (on a transient fetch error → NowState::default()) blank the footer
                // mid-action. The in-flight action's own completion reconciles.
                if self.pending_transport.is_some() {
                    return Task::none();
                }
                // A fresh authoritative state with no action pending: any standing
                // retry banner is now moot (the daemon answered + this is the truth),
                // so dismiss it — otherwise a transport that actually applied
                // server-side after a client-side timeout would leave the banner
                // stuck, contradicting the visible state.
                self.failed_transport = None;
                let changed = s.song_id != self.now_state.song_id;
                self.now_state = s;
                if changed {
                    // MOTION-FEEDBACK — a fresh now-playing track gently reveals
                    // the footer (fade-and-rise; crossfade under reduce-motion).
                    if self.now_state.has_track() || self.now_state.active {
                        self.now_reveal = Some(motion::Reveal::starting_at(
                            std::time::Instant::now(),
                            self.reduce_motion,
                        ));
                    }
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
                // MOTION-NET-4 — apply optimistically, then issue with revert/retry.
                // `begin_transport` snapshots the PRE-action state (for the revert)
                // before this flip is applied, so capture `prior` first.
                let prior = self.now_state.clone();
                self.now_state.volume = v;
                self.begin_transport(TransportAction::SetVolume { level: v }, prior)
            }
            // MUSIC-RFX-4 — scrub: jump the playhead optimistically so the slider
            // tracks the drag, then tell the daemon to seek (RFX-2).
            Message::Seek(ms) => {
                let prior = self.now_state.clone();
                self.now_state.position_ms = ms;
                self.begin_transport(TransportAction::Seek { position_ms: ms }, prior)
            }
            Message::PlayPause => {
                // MUSIC-RESPONSIVE-8 — optimistic: flip the play icon immediately,
                // then reconcile from the real state on TransportDone. `play_pause`
                // takes the PRE-flip state to decide the action.
                let prior = self.now_state.clone();
                let was = self.now_state.playing;
                self.now_state.playing = !was;
                self.begin_transport(TransportAction::PlayPause { was_playing: was }, prior)
            }
            Message::SkipNext => {
                // MUSIC-RESPONSIVE-8 — a skip keeps playing; show that immediately,
                // the new track title reconciles on TransportDone.
                let prior = self.now_state.clone();
                self.now_state.playing = true;
                self.begin_transport(TransportAction::SkipNext, prior)
            }
            Message::SkipPrev => {
                let prior = self.now_state.clone();
                self.now_state.playing = true;
                self.begin_transport(TransportAction::SkipPrev, prior)
            }
            // MOTION-NET-4 — the transport RPC resolved. The pure
            // `resolve_transport_outcome` decides: reconcile from the real daemon
            // state on success (clearing the pending/failed slots), revert the
            // optimistic flip to the pre-action snapshot + arm the retry banner on
            // failure (the footer keeps its context, never blanks), or ignore a
            // stale completion that a newer action already superseded.
            Message::TransportDone(action, result) => {
                match resolve_transport_outcome(self.pending_transport.clone(), action, result) {
                    TransportOutcome::Reconcile => {
                        self.pending_transport = None;
                        self.failed_transport = None;
                        Task::perform(nowplaying::fetch_state(), |r| {
                            Message::StateLoaded(r.unwrap_or_default())
                        })
                    }
                    TransportOutcome::Revert { snapshot, action } => {
                        self.pending_transport = None;
                        self.now_state = snapshot;
                        self.failed_transport = Some(action);
                        Task::none()
                    }
                    // Stale: a newer action owns the state + pending slot — leave
                    // `pending_transport` intact so the newer action's own
                    // completion still finds its pending entry.
                    TransportOutcome::Ignore => Task::none(),
                }
            }
            // MOTION-NET-4 — the retry banner button: re-apply + re-issue the last
            // failed action against the current state (so the optimistic flip + the
            // revert/retry loop hold across repeated failures).
            Message::RetryTransport => {
                let Some(action) = self.failed_transport.take() else {
                    return Task::none();
                };
                let prior = self.now_state.clone();
                // Re-issue the EXACT failed action (same verb/target), re-applying
                // its optimistic effect. We reuse the stored action verbatim — e.g.
                // PlayPause carries the original `was_playing`, so a background poll
                // that flipped `now_state` between the revert and this click can't
                // make the retry send the opposite verb (the documented contract).
                match action {
                    TransportAction::PlayPause { was_playing } => {
                        // The optimistic icon should reflect the action's intent:
                        // resume (`was_playing=false`) ⇒ show playing, and vice versa.
                        self.now_state.playing = !was_playing;
                    }
                    TransportAction::SkipNext | TransportAction::SkipPrev => {
                        self.now_state.playing = true;
                    }
                    TransportAction::SetVolume { level } => self.now_state.volume = level,
                    TransportAction::Seek { position_ms } => {
                        self.now_state.position_ms = position_ms
                    }
                }
                self.begin_transport(action, prior)
            }
            // MOTION-FEEDBACK — clear any reveal that has finished so the tick
            // subscription disarms (zero idle wakeups). The repaint this tick
            // triggers re-reads the live `slide_in` params for in-flight reveals.
            Message::RevealTick => {
                let now = std::time::Instant::now();
                if self.now_reveal.is_some_and(|r| !r.is_animating(now)) {
                    self.now_reveal = None;
                }
                if self.queue_reveal.is_some_and(|r| !r.is_animating(now)) {
                    self.queue_reveal = None;
                }
                Task::none()
            }
            // BEAUT-MUSIC — clear the mount once it settles so its tick disarms
            // (zero idle wakeups); the repaint this tick triggers re-reads the
            // live `slide_in` params for an in-flight mount.
            Message::MountTick => {
                if self
                    .mount
                    .is_some_and(|m| !m.is_animating(std::time::Instant::now()))
                {
                    self.mount = None;
                }
                Task::none()
            }
            // BEAUT-MUSIC — a no-op state change whose sole purpose is to force a
            // repaint so an on-screen breathing skeleton re-reads its live alpha.
            // Armed only while a skeleton is visible and motion is on, so it
            // self-disarms the instant content lands (the subscription gate).
            Message::ShimmerTick => Task::none(),
        }
    }

    /// BEAUT-MUSIC — is a breathing skeleton placeholder currently on screen?
    /// True while the daemon state behind the active surface is still loading:
    /// the Home dashboard before its first stats batch lands, or a library grid
    /// mid-fetch. Gates the shimmer tick so the breathe costs nothing once real
    /// content paints (MOTION-PERF-1). The first-run welcome card is NOT a
    /// skeleton (it's the final content), so it never arms the shimmer.
    fn skeleton_visible(&self) -> bool {
        if self.form.is_some() || self.maxi_open {
            return false;
        }
        // POLISH-music-errorretry — the search results overlay sits above whatever
        // route is behind it, so its loading skeleton breathes whenever a search
        // fetch is in flight, independent of the route.
        if self.searching {
            return true;
        }
        match self.nav.current() {
            // Home shows the stat/server/chip skeleton until the first batch.
            Route::Hub => self.stats.is_none(),
            // POLISH-music-errorretry — the album / playlist detail pages now show
            // the breathing track-list skeleton while their fetch is in flight
            // (replacing the old static text line), so arm the ticker for them too.
            Route::Album(..) => self.album_loading,
            Route::Playlist(..) => self.playlist_loading,
            // A category grid shows the skeleton tiles while fetching.
            _ => self.loading,
        }
    }

    /// MOTION-NET-4 — record `action` as the in-flight optimistic transport
    /// (with `prior` = the `now_state` captured BEFORE the optimistic flip, for a
    /// revert) and issue its Bus call. Any standing retry banner is cleared — a
    /// fresh attempt is now in flight; it re-arms only if THIS attempt also fails.
    /// The caller has already applied the optimistic state change.
    fn begin_transport(&mut self, action: TransportAction, prior: NowState) -> Task<Message> {
        self.pending_transport = Some((action, prior));
        self.failed_transport = None;
        action.perform()
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
        // MOTION-FEEDBACK — the now-playing / queue reveal ticker, armed ONLY
        // while a reveal is in flight (MOTION-PERF-1 — a settled surface costs no
        // idle wakeups). Disarms when `RevealTick` clears the finished reveals.
        let now = std::time::Instant::now();
        let revealing = self.now_reveal.is_some_and(|r| r.is_animating(now))
            || self.queue_reveal.is_some_and(|r| r.is_animating(now));
        let reveal = if revealing {
            cosmic::iced::time::every(motion::REVEAL_TICK).map(|_| Message::RevealTick)
        } else {
            Subscription::none()
        };
        // BEAUT-MUSIC — the first-paint mount ticker, armed ONLY while the welcome
        // / dashboard fade-and-rise is in flight. Disarms via `MountTick` the
        // instant it settles (zero idle wakeups; reduce-motion settles in ≤80 ms).
        let mount = if self.mount.is_some_and(|m| m.is_animating(now)) {
            cosmic::iced::time::every(SLIDE_TICK).map(|_| Message::MountTick)
        } else {
            Subscription::none()
        };
        // BEAUT-MUSIC — the skeleton shimmer ticker, armed ONLY while a breathing
        // placeholder is on screen AND motion is live (under reduce-motion / the
        // kill switch the grey is static, so no tick) — the shared
        // `SkeletonShimmer::needs_tick(visible)` predicate, fed the surface's
        // visibility. It self-disarms the instant real content lands.
        let shimmer = if self.shimmer.needs_tick(self.skeleton_visible()) {
            cosmic::iced::time::every(motion::SHIMMER_TICK).map(|_| Message::ShimmerTick)
        } else {
            Subscription::none()
        };
        // Poll the now-playing snapshot once the library is shown (there's
        // no daemon to ask on the first-run connect form).
        if self.form.is_some() {
            Subscription::batch([keys, resizes, slide, reveal, mount, shimmer])
        } else {
            let mut subs = vec![
                keys,
                resizes,
                slide,
                reveal,
                mount,
                shimmer,
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

    /// MUSIC-DOCK-1/2/5 — map the dock as a full-height Overlay surface anchored
    /// on all four edges (top+bottom+left+right) and start the slide-up. **No
    /// exclusive zone** (DOCK-5 — it overlays the desktop and never reserves space
    /// / reshapes other windows). `OnDemand` keyboard so its buttons + search
    /// field take focus on click; no titlebar (a layer surface has none). The
    /// surface is created with its content pushed down (a positive top margin) so
    /// the first slide tick reveals it rising into place.
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
        // MUSIC-DOCK-NORENDER — start the dock's top edge DOCK_SLIDE_PX down (a
        // positive top margin) so the entrance rises into place on the slide
        // ticks (under reduce-motion the tween is ~instant, so it snaps to 0 on
        // the first tick — the dock effectively maps in place). On a four-edge-
        // anchored surface a margin offsets only its own edge, so the top margin
        // is what shifts the visible content; see `Message::SlideTick`.
        let start_top = DOCK_SLIDE_PX.round() as i32;
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            namespace: "mde-music".to_string(),
            // MUSIC-DOCK-NORENDER (2026-06-24) — full-height dock: anchor ALL FOUR
            // edges (top+bottom+left+right), exactly like `mde-mesh-wallpaper`.
            // A layer surface only gets a non-zero extent on an axis when BOTH of
            // that axis's edges are anchored *or* a fixed size is given for it; a
            // surface anchored only bottom+left+right with `size: (None, None)`
            // has its vertical axis unspanned and unsized, so the compositor maps
            // it at ZERO height — the dock came up invisible (opening "Music"
            // rendered nothing). Anchoring top+bottom spans the vertical axis so
            // the compositor sizes it to the output; the DOCK-2 slide-up still
            // works, now by animating the TOP margin (on a four-edge-anchored
            // surface a margin offsets only its own edge, so the top margin is
            // what shifts the visible content down/up — see `Message::SlideTick`).
            anchor: Anchor::TOP
                .union(Anchor::BOTTOM)
                .union(Anchor::LEFT)
                .union(Anchor::RIGHT),
            // All four edges anchored → the compositor fills both axes; no fixed
            // size needed (mirrors the wallpaper's `size: None`).
            size: None,
            // MUSIC-DOCK-5 — overlay only; reserve NO space (0 = don't push other
            // surfaces). Distinct from the notify-center, which DOES reserve.
            // Anchoring all four edges does NOT reserve space — only a non-zero
            // (or -1) exclusive_zone does — so the dock still just overlays.
            exclusive_zone: 0,
            layer: Layer::Overlay,
            // Interactive: buttons + the search field need clicks/focus.
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            margin: cosmic::iced::platform_specific::runtime::wayland::layer_surface::IcedMargin {
                top: start_top,
                right: 0,
                bottom: 0,
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

    /// BEAUT-MUSIC — the first-open **welcome** view: a Carbon-styled connect
    /// card (raised surface + 1px border + accent hero glyph), centered in the
    /// dock instead of a bare top-left form, so the very first frame reads as a
    /// designed onboarding pane rather than a blank one. Every colour is an
    /// `mde-theme` token (§4); the whole card gently fades-and-rises in on the
    /// first-paint [`motion::MountReveal`] (a pure crossfade under reduce-motion),
    /// and the Connect CTA carries the shared hover/press feedback.
    fn first_run_view(&self, f: &FirstRunForm) -> Element<'_, Message> {
        let p = mde_theme::Palette::dark();
        let text_c = carbon(p.text, 1.0);
        let muted = carbon(p.text_muted, 1.0);
        let accent = carbon(p.accent, 1.0);

        let hero = container(text("\u{266B}").size(40).colr(accent))
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .center_x(Length::Fixed(72.0))
            .center_y(Length::Fixed(72.0))
            .style({
                let bg = carbon(p.raised, 1.0);
                move |_| cosmic::iced::widget::container::Style {
                    background: Some(bg.into()),
                    border: cosmic::iced::Border {
                        color: cosmic::iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: 12.0.into(),
                    },
                    ..Default::default()
                }
            });

        let mut col = column![
            hero,
            Space::new().height(Length::Fixed(16.0)),
            text("Welcome to MCNF Music").size(22).colr(text_c),
            Space::new().height(Length::Fixed(6.0)),
            text("Connect your Airsonic / Navidrome server to start listening across the mesh.")
                .size(13)
                .colr(muted),
            Space::new().height(Length::Fixed(20.0)),
            text_input("https://music.your-mesh:4040", &f.url).on_input(Message::UrlChanged),
            text_input("username", &f.user).on_input(Message::UserChanged),
            text_input("password", &f.pass)
                .secure(true)
                .on_input(Message::PassChanged),
            Space::new().height(Length::Fixed(16.0)),
            // The CTA carries the shared hover-lift / press-depress feedback.
            primary_button("Connect", 14, Message::Connect, self.reduce_motion),
        ]
        .spacing(8)
        .padding(32)
        .max_width(420)
        .align_x(cosmic::iced::Alignment::Center);
        // MEDIA-8 — when creds already exist this form is the "change server"
        // OVERRIDE (not first-run), so offer a Cancel to return to browsing. A
        // genuinely unconfigured node (no creds) has nothing to return to, so no
        // Cancel is shown — the form stays the gate there.
        if creds::load().is_ok() {
            col = col.push(Space::new().height(Length::Fixed(4.0)));
            col = col.push(
                button(text("Cancel").size(13).colr(muted))
                    .on_press(Message::CancelChangeServer)
                    .padding([6, 12]),
            );
        }
        if let Some(err) = &f.error {
            col = col.push(Space::new().height(Length::Fixed(8.0)));
            col = col.push(text(err.clone()).size(13).colr(carbon(p.danger, 1.0)));
        }

        // Carbon card: raised surface + 1px border, centered in the dock.
        let card = container(col).style({
            let bg = carbon(p.surface, 1.0);
            let border = carbon(p.border, 1.0);
            move |_| cosmic::iced::widget::container::Style {
                background: Some(bg.into()),
                border: cosmic::iced::Border {
                    color: border,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            }
        });
        // BEAUT-MUSIC — gently rise the welcome card into place on first open:
        // the mount's translate_y becomes a top-padding offset (the established
        // translate→padding glue). The card stays fully opaque so the connect
        // form is usable from the first frame; under reduce-motion `slide_in`
        // yields no rise, so it simply maps in place.
        let rv = motion::mount_params(self.mount, std::time::Instant::now());
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(cosmic::iced::Padding {
                top: rv.translate_y.max(0.0),
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
            .into()
    }

    /// MUSIC-LOCK-FIX — fetch cover art only for the grid window around scroll
    /// offset `offset_y`, bounded so a large library can't fan out hundreds of
    /// fetches (each of which re-renders the whole grid → the UI lock). Skips
    /// already-cached + already-requested ids; marks the rest requested. The
    /// row pitch + column count are the single-sourced Carbon grid metrics
    /// (MUSIC-RFX-10) shared with the grid in [`Self::library_view`].
    fn art_window_task(&mut self, offset_y: f32) -> Task<Message> {
        const WINDOW_ROWS: usize = 14; // ~2 screenfuls + buffer
        let gm = GridMetrics::carbon_dense();
        let row_pitch = gm.row_pitch();
        let cols = gm.columns_for_width(self.grid_width);
        let start_row = ((offset_y / row_pitch).floor() as usize).saturating_sub(2);
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
        // accent avatar + the live Airsonic connection line. MEDIA-8 — clicking
        // it opens the "change server" override form (pre-filled from the saved
        // creds), the manual escape hatch now that auto-config browses by default.
        col = col.push(Space::new().height(Length::Fill));
        let conn = if self.connection.is_empty() {
            "Not connected".to_string()
        } else {
            self.connection.clone()
        };
        let avatar = container(
            text(mde_theme::Icon::StatusOk.fallback_glyph())
                .size(14)
                .colr(carbon(p.accent, 1.0)),
        )
        .width(Length::Fixed(28.0))
        .center_x(Length::Fixed(28.0));
        let account = button(
            row![avatar, text(conn).size(12).colr(carbon(p.text_muted, 1.0))]
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center),
        )
        .on_press(Message::ChangeServer)
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
            // BEAUT-MUSIC — a breathing Carbon skeleton (title bar · hero counts ·
            // server line · count chips) while the first `library-stats` batch is
            // in flight, so Home paints its structure within one frame instead of
            // a bare "Loading…" line. The fill is the shared
            // `SkeletonShimmer::fill` (the palette `text` token at the live shimmer
            // alpha) — static grey under reduce-motion; the tick gates on
            // `skeleton_visible`.
            let fill = self.shimmer.fill(std::time::Instant::now(), &p);
            let hero_block = row![
                skeleton_bar(90, 40, fill),
                skeleton_bar(90, 40, fill),
                skeleton_bar(90, 40, fill),
            ]
            .spacing(40);
            return column![
                skeleton_bar(180, 24, fill),
                Space::new().height(Length::Fixed(18.0)),
                hero_block,
                Space::new().height(Length::Fixed(22.0)),
                skeleton_bar(260, 16, fill),
                Space::new().height(Length::Fixed(8.0)),
                skeleton_bar(320, 12, fill),
                Space::new().height(Length::Fixed(20.0)),
                skeleton_bar(360, 14, fill),
            ]
            .spacing(0)
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
                text(mde_theme::Icon::StatusOk.fallback_glyph())
                    .size(12)
                    .colr(dotc),
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
                    (
                        mde_theme::Icon::Audio.fallback_glyph(),
                        carbon(p.accent, 1.0),
                    )
                } else {
                    (mde_theme::Icon::StatusUnknown.fallback_glyph(), muted)
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
        // BEAUT-MUSIC — staged reveal: when the first stats batch replaced the
        // skeleton, the dashboard fades-and-rises into place (a fresh mount
        // epoch set in `StatsLoaded`). The translate_y becomes a top-padding
        // rise; under reduce-motion `slide_in` yields a pure crossfade (no rise).
        let rv = motion::mount_params(self.mount, std::time::Instant::now());
        let body = container(body).padding(cosmic::iced::Padding {
            top: rv.translate_y.max(0.0),
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });
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
                crumbs = crumbs.push(text(mde_theme::Icon::ChevronRight.fallback_glyph()));
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
                let route_pal = mde_theme::Palette::dark();
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
                // MUSIC-RFX-10 — single-sourced Carbon card-grid metrics (card
                // width / art tile / gutter / row pitch all from mde-theme tokens).
                let gm = GridMetrics::carbon_dense();
                // AIR-11.c — width-adaptive column count (shared by the skeleton +
                // the real grid so the loading placeholder matches the layout).
                let cols = gm.columns_for_width(self.grid_width);
                if self.loading {
                    // MUSIC-RESPONSIVE-6 / BEAUT-MUSIC — breathing Carbon skeleton
                    // tiles (matching the card geometry) instead of a blank
                    // "Loading…" line, so a navigation paints structure within one
                    // frame and the slow load reads as active (static under
                    // reduce-motion).
                    let fill = self.shimmer.fill(std::time::Instant::now(), &route_pal);
                    col = col.push(skeleton_grid(cols, fill));
                } else if let Some(err) = &self.load_error {
                    // POLISH-music-errorretry — render the failure through the
                    // shared Carbon LoadState (icon + label + tone) with a live
                    // Retry wired to RetryLoad, instead of a bare red one-liner.
                    col = col.push(load_error_block(
                        load_state_for_error(err),
                        err,
                        ListMetrics::carbon_dense(),
                        Some(Message::RetryLoad),
                    ));
                } else if self.items.is_empty() {
                    // BEAUT-MUSIC — a tasteful Carbon empty state (hero glyph +
                    // heading + body) instead of a bare one-liner.
                    // POLISH-music-playlistcreate — the Playlists page pairs its
                    // empty state with the inline create form so a zero-playlist
                    // operator can make their first one. Without it the page (and
                    // the add-to-playlist sheet's "create one on the Playlists
                    // page" hint) dead-ends.
                    let (heading, body, show_create) = empty_state_for(route);
                    col = col.push(empty_state(heading, body));
                    if show_create {
                        col = col.push(container(self.new_playlist_form()).center_x(Length::Fill));
                    }
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
                    // AIR-11.c — width-adaptive grid: the column count is derived
                    // from the live window width (tracked via the WindowResized
                    // subscription) so the 160px cards reflow on resize, replacing
                    // the AIR-11.b fixed 5-column layout. Per-card cover art and
                    // scroll-position persistence are wired below (art_cache /
                    // grid_scroll).
                    // MUSIC-RESPONSIVE-9 — virtualize large grids: render only the
                    // visible row window (+overscan) and reserve the off-window
                    // height with spacers, so a multi-hundred-card library doesn't
                    // build every card per frame. Skipped for the Playlists page
                    // (variable-height cards + inline forms) and small grids (where
                    // full render is cheaper than the windowing bookkeeping). Same
                    // row pitch / window as `art_window_task`, and the live scroll
                    // offset comes from `grid_scroll` (kept current by GridScrolled).
                    let row_pitch = gm.row_pitch();
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
                        ((offset_y / row_pitch).floor() as usize).saturating_sub(2)
                    } else {
                        0
                    };
                    let end_row = if virtualize {
                        (start_row + WINDOW_ROWS).min(total_rows)
                    } else {
                        total_rows
                    };
                    let mut grid = column![].spacing(gm.gap);
                    if virtualize && start_row > 0 {
                        grid = grid
                            .push(Space::new().height(Length::Fixed(start_row as f32 * row_pitch)));
                    }
                    for (row_idx, chunk) in items.chunks(cols).enumerate() {
                        if virtualize && (row_idx < start_row || row_idx >= end_row) {
                            continue;
                        }
                        let mut r = row![].spacing(gm.gap);
                        for item in chunk {
                            // MUSIC-ALBUMS-3 — Carbon album card: square art tile
                            // (raised fill + ♪ placeholder until art loads) over
                            // a 2-line title.
                            let cpal = mde_theme::Palette::dark();
                            let art_inner: Element<'_, Message> =
                                if let Some(handle) = self.art_cache.get(&item.id) {
                                    image(handle.clone())
                                        .width(Length::Fill)
                                        .height(Length::Fixed(gm.art_height))
                                        .into()
                                } else {
                                    container(
                                        text("\u{266A}")
                                            .size(gm.placeholder_glyph)
                                            .colr(carbon(cpal.text_muted, 1.0)),
                                    )
                                    .center_x(Length::Fill)
                                    .center_y(Length::Fixed(gm.art_height))
                                    .into()
                                };
                            let art = container(art_inner)
                                .width(Length::Fill)
                                .height(Length::Fixed(gm.art_height))
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
                                    .size(gm.title)
                                    .colr(carbon(cpal.text, 1.0)),
                            ]
                            .spacing(gm.gap)
                            .into();
                            let mut btn = button(card_content)
                                .width(Length::Fixed(gm.card_width))
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
                                        button(
                                            text(mde_theme::Icon::Confirm.fallback_glyph())
                                                .size(12),
                                        )
                                        .on_press(Message::CommitRenamePlaylist),
                                        button(
                                            text(mde_theme::Icon::Cancel.fallback_glyph()).size(12),
                                        )
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
                                        .width(Length::Fixed(gm.card_width)),
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
                                .height(Length::Fixed((total_rows - end_row) as f32 * row_pitch)),
                        );
                    }
                    // MUSIC-RFX-6 — the Playlists page gets a "new playlist" form.
                    if matches!(route, Route::Category(HubCard::Playlists)) {
                        col = col.push(self.new_playlist_form());
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

        let pal = mde_theme::Palette::dark();
        // MUSIC-ICONS — a leading Carbon search glyph (the typed `Icon::Search`,
        // single-sourced from `mde_theme`) marks the field as a search affordance.
        let search_field = row![
            text(mde_theme::Icon::Search.fallback_glyph())
                .size(13)
                .colr(carbon(pal.text, 1.0)),
            // POLISH-music-feedback — the cosmic-native text input (vs the iced
            // fork one the dialog fields use) so the surface's one keyboard-focusable
            // control exposes a `focused` style hook; `search_input_style` draws the
            // shared 2px Carbon focus ring around it on keyboard focus. It registers
            // focusable under the same id, so Cmd-F (`FocusSearch`) is unchanged.
            cosmic::widget::text_input("Search artists, albums, songs…", &self.search_query)
                .id(search_id())
                .on_input(Message::SearchInput)
                .style(search_input_style(self.reduce_motion))
                .padding(8)
                .width(Length::Fixed(340.0)),
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center);
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

    /// MUSIC-RFX-6 / POLISH-music-playlistcreate — the inline "new playlist"
    /// form (name field + Create). Shared by the populated Playlists grid and
    /// its empty state, so a zero-playlist operator always has a way to create
    /// the first one (single-sourced here rather than duplicated, §6).
    fn new_playlist_form(&self) -> Element<'_, Message> {
        row![
            text_input("New playlist name…", &self.new_playlist_name)
                .on_input(Message::NewPlaylistNameChanged)
                .on_submit(Message::CreatePlaylist)
                .width(Length::Fixed(280.0)),
            button(text("Create").size(13)).on_press(Message::CreatePlaylist),
        ]
        .spacing(8)
        .into()
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
        // MUSIC-RFX-10 — Carbon dense result rows from mde-theme tokens.
        let m = ListMetrics::carbon_dense();
        let mut col = column![text("Search").size(m.heading)]
            .spacing(m.header_gap)
            .padding(m.pad)
            .max_width(720);
        if self.searching {
            // POLISH-music-errorretry — a breathing list skeleton while the query
            // runs, the shared Carbon convention, instead of a bare "Searching…".
            let fill = self
                .shimmer
                .fill(std::time::Instant::now(), &mde_theme::Palette::dark());
            col = col.push(loading_list_skeleton(fill, m, 4));
        } else if let Some(err) = &self.search_error {
            // The search overlay is not a route, so its Retry re-runs the current
            // query (SearchTick) rather than going through `reload_current`.
            col = col.push(load_error_block(
                load_state_for_error(err),
                err,
                m,
                Some(Message::SearchTick(self.search_seq)),
            ));
        } else if let Some(results) = &self.search_results {
            if results.is_empty() {
                col = col.push(text("No results.").size(m.body));
            } else {
                col = col.push(result_section("Artists", &results.artists, m, |it| {
                    Message::OpenArtist(it.id.clone(), it.label.clone())
                }));
                col = col.push(result_section("Albums", &results.albums, m, |it| {
                    Message::OpenAlbum(it.id.clone(), it.label.clone())
                }));
                col = col.push(result_section("Songs", &results.songs, m, |it| {
                    Message::EnqueueSong(it.id.clone())
                }));
            }
        }
        col = col.push(Space::new().height(Length::Fixed(f32::from(m.pad))));
        col = col.push(button(text("Close")).on_press(Message::DismissSearch));
        container(scrollable(col))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(40)
            .into()
    }

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
        // MUSIC-RFX-10 — shared Carbon dense list metrics.
        let m = ListMetrics::carbon_dense();
        let header = column![
            text(name.clone()).size(m.heading),
            text(format!(
                "{} track(s) · reorder with ↑/↓",
                self.playlist_tracks.len()
            ))
            .size(m.mono),
            Space::new().height(Length::Fixed(f32::from(m.header_gap))),
            button(text("Play all")).on_press(Message::PlayPlaylist(id.clone())),
        ]
        .spacing(m.row_gap);

        let mut list = column![].spacing(m.row_gap);
        if self.playlist_loading {
            // POLISH-music-errorretry — tracks are still loading: show the
            // breathing skeleton, NOT the "empty" line (which would be a lie until
            // the fetch settles).
            let fill = self
                .shimmer
                .fill(std::time::Instant::now(), &mde_theme::Palette::dark());
            list = list.push(loading_list_skeleton(fill, m, 6));
        } else if self.playlist_tracks.is_empty() {
            list = list.push(text("This playlist is empty.").size(m.body));
        }
        let last = self.playlist_tracks.len().saturating_sub(1);
        for (i, t) in self.playlist_tracks.iter().enumerate() {
            let mut row_el = row![
                text(format!("{}.", i + 1))
                    .size(m.body)
                    .width(Length::Fixed(m.number_col)),
                text(t.label.clone()).size(m.body).width(Length::Fill),
            ]
            .spacing(m.col_gap)
            .align_y(cosmic::iced::Alignment::Center);
            if i > 0 {
                row_el = row_el
                    .push(button(text("↑").size(m.mono)).on_press(Message::PlaylistMoveUp(i)));
            }
            if i < last {
                row_el = row_el
                    .push(button(text("↓").size(m.mono)).on_press(Message::PlaylistMoveDown(i)));
            }
            // MUSIC-RFX-8 — right-click for the action menu (incl. Remove).
            let menu_ctx = TrackContext {
                song_id: t.id.clone(),
                title: t.label.clone(),
                playlist_index: Some(i),
            };
            list = list.push(mouse_area(row_el).on_right_press(Message::OpenTrackMenu(menu_ctx)));
        }

        column![
            header,
            Space::new().height(Length::Fixed(f32::from(m.header_gap))),
            list
        ]
        .spacing(m.row_gap)
        .padding(m.pad)
        .into()
    }

    /// AIR-12 — the album detail page: a cover-art column (the image once
    /// art-over-Bus resolves it, a glyph until then) + the album header
    /// (Play / Shuffle / Add) + the numbered track list (each row can Play-Next
    /// or Add-to-Queue).
    fn album_page(&self) -> Element<'_, Message> {
        if self.album_loading {
            // POLISH-music-errorretry — a breathing header + track-list skeleton
            // while the album fetches, the shared Carbon convention, instead of a
            // bare "Loading album…" line.
            let fill = self
                .shimmer
                .fill(std::time::Instant::now(), &mde_theme::Palette::dark());
            return album_loading_skeleton(fill);
        }
        if let Some(err) = &self.album_error {
            // POLISH-music-errorretry — shared LoadState failure block with a live
            // Retry (RetryLoad now re-fetches the album route), not a bare line.
            return load_error_block(
                load_state_for_error(err),
                err,
                ListMetrics::carbon_dense(),
                Some(Message::RetryLoad),
            );
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
        // MUSIC-RFX-10 — Carbon dense list metrics (gaps/widths/sizes from
        // mde-theme tokens, no scattered literals).
        let m = ListMetrics::carbon_dense();
        let header = column![
            text(a.name.clone()).size(m.heading),
            text(a.artist.clone()).size(m.body),
            text(meta).size(m.mono),
            Space::new().height(Length::Fixed(f32::from(m.header_gap))),
            actions,
        ]
        .spacing(m.row_gap);

        // Numbered track rows with per-track Play-Next / Add-to-Queue.
        let mut list = column![].spacing(m.row_gap);
        for (i, t) in a.tracks.iter().enumerate() {
            let no = t
                .track_no
                .unwrap_or_else(|| u32::try_from(i + 1).unwrap_or(0));
            let track_row = row![
                text(format!("{no}."))
                    .size(m.body)
                    .width(Length::Fixed(m.number_col)),
                text(t.title.clone()).size(m.body).width(Length::Fill),
                text(album::fmt_duration(t.duration))
                    .size(m.mono)
                    .width(Length::Fixed(m.duration_col)),
                button(text("Play Next").size(m.caption))
                    .on_press(Message::PlayTrackNext(t.id.clone())),
                button(text("+ Queue").size(m.caption))
                    .on_press(Message::AddTrackToQueue(t.id.clone())),
                // MUSIC-RFX-7 — add this track to a playlist.
                button(text("+ Playlist").size(m.caption))
                    .on_press(Message::OpenAddToPlaylist(t.id.clone())),
            ]
            .spacing(m.col_gap)
            .align_y(cosmic::iced::Alignment::Center);
            // MUSIC-RFX-8 — right-click the row for the dense action menu.
            let menu_ctx = TrackContext {
                song_id: t.id.clone(),
                title: t.title.clone(),
                playlist_index: None,
            };
            list =
                list.push(mouse_area(track_row).on_right_press(Message::OpenTrackMenu(menu_ctx)));
        }

        // Cover image (left) over header/tracks (right): the resolved art-over-Bus
        // image when present, a glyph placeholder until it loads.
        let art: Element<'_, Message> = match &self.album_art {
            Some(handle) => image(handle.clone())
                .width(Length::Fixed(220.0))
                .height(Length::Fixed(220.0))
                .into(),
            None => container(text(mde_theme::Icon::Audio.fallback_glyph()).size(48))
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
            Space::new().height(Length::Fixed(f32::from(m.header_gap))),
            scrollable(list)
        ]
        .spacing(m.pad)
        .width(Length::Fill);
        row![art, content].spacing(m.col_gap).into()
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
        // MOTION-FEEDBACK — the footer's gentle reveal (fade-and-rise) when a
        // fresh track loads. The alpha tints the metadata text; the translate_y
        // becomes a top-padding offset on the bar container below.
        let rv = motion::reveal_params(self.now_reveal, std::time::Instant::now(), 0);
        let p = mde_theme::Palette::dark();
        let muted = carbon(p.text_muted, rv.alpha);
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
            text(title).size(13).colr(carbon(p.text, rv.alpha)),
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
        let rm = self.reduce_motion;
        let bar = row![
            mini,
            meta,
            // MOTION-FEEDBACK — the shuttle controls carry the shared hover-lift /
            // press-depress (press fires on down, no delay; reduce-motion keeps the
            // state change without movement).
            transport_button("\u{25C0}", 13, Message::SkipPrev, rm),
            transport_button(play_pause, 13, Message::PlayPause, rm),
            transport_button("\u{25B6}", 13, Message::SkipNext, rm),
            pos,
            // Audio routing — send playback to a mesh peer (AIR-8 take-over).
            transport_button("\u{21C6} Route", 12, Message::OpenRouting, rm),
            transport_button("Full", 12, Message::ToggleMaxi, rm),
        ]
        .spacing(10)
        .padding(10)
        .align_y(cosmic::iced::Alignment::Center);
        // MOTION-FEEDBACK — the translate_y becomes a top-padding offset (the
        // established translate→padding glue), so the bar rises into place. Under
        // reduce-motion `slide_in` collapses to a pure crossfade (no rise).
        let bar = container(bar)
            .width(Length::Fill)
            .padding(cosmic::iced::Padding {
                top: rv.translate_y.max(0.0),
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            });
        // MOTION-NET-4 — a failed transport action surfaces a non-blocking retry
        // banner pinned above the controls (the optimistic flip has already been
        // reverted, so the bar below still shows the real prior track + state).
        let stacked: Element<'_, Message> = match self.retry_banner() {
            Some(banner) => column![banner, bar].into(),
            None => bar.into(),
        };
        Some(stacked)
    }

    /// MOTION-NET-4 — the non-blocking retry banner: shown when the last transport
    /// action failed (the optimistic flip was already reverted to its pre-action
    /// snapshot). It names what failed (amber warning token) and offers a Retry
    /// that re-issues the exact action. `None` when the last action succeeded.
    fn retry_banner(&self) -> Option<Element<'_, Message>> {
        let action = self.failed_transport?;
        let p = mde_theme::Palette::dark();
        let warning = carbon(p.warning, 1.0);
        let label = text(action.failed_label())
            .size(12)
            .colr(warning)
            .width(Length::Fill);
        let retry = button(text("Retry").size(12).colr(carbon(p.text, 1.0)))
            .on_press(Message::RetryTransport)
            .padding([4, 12]);
        let row = row![label, retry]
            .spacing(10)
            .align_y(cosmic::iced::Alignment::Center);
        // A subtle raised band so the banner reads as a transient status strip,
        // not part of the controls (tokens only — §4).
        Some(
            container(row)
                .width(Length::Fill)
                .padding([6, 12])
                .style(move |_| cosmic::iced::widget::container::Style {
                    background: Some(carbon(p.raised, 1.0).into()),
                    ..Default::default()
                })
                .into(),
        )
    }

    /// AIR-15.b — the maxi-player full-window surface: a scaling cover-art hero
    /// over a dominant-colour tint band, the now-playing header (title/artist +
    /// transport), the scrub bar + volume slider, and the Queue / Lyrics / Peers
    /// tabs.
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
                    // MOTION-FEEDBACK — the maxi shuttle controls share the
                    // hover-lift / press-depress vocabulary (press on down, no
                    // delay; reduce-motion keeps the state change without movement).
                    transport_button("Prev", 13, Message::SkipPrev, self.reduce_motion),
                    transport_button(play_pause, 13, Message::PlayPause, self.reduce_motion),
                    transport_button("Next", 13, Message::SkipNext, self.reduce_motion),
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
        // MOTION-NET-4 — surface the same retry banner in the maxi view, so a
        // failed transport from the full surface is recoverable in place too.
        let header = match self.retry_banner() {
            Some(banner) => column![top_bar, banner, hero].spacing(12),
            None => column![top_bar, hero].spacing(12),
        };

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
        // MUSIC-RFX-9 — virtualize the queue: render only the visible row window
        // (+overscan) and reserve the off-window height with spacers, so a queue of
        // thousands of tracks doesn't build every row per frame. Same spacer pattern
        // as the library grid (RESPONSIVE-9); `queue_scroll` is kept live by
        // `QueueScrolled`. A short queue renders in full (`virtualize=false`).
        let win = QueueWindow::resolve(self.queue_songs.len(), self.queue_scroll);
        if win.virtualize && win.start > 0 {
            queue =
                queue.push(Space::new().height(Length::Fixed(win.start as f32 * QUEUE_ROW_PITCH)));
        }
        // MOTION-FEEDBACK — a (re)loaded queue reveals its rows top-down: each
        // row's staggered slide-in (queue_reveal) gives a leading-padding rise +
        // a label fade. Settled (or absent) reveals read at rest.
        let reveal_now = std::time::Instant::now();
        for (i, sid) in self.queue_songs.iter().enumerate() {
            // Skip rows outside the mounted window; their height is held by the
            // leading/trailing spacers so the scrollbar geometry is unchanged.
            if win.virtualize && (i < win.start || i >= win.end) {
                continue;
            }
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
            let rv = motion::reveal_params(
                self.queue_reveal,
                reveal_now,
                u32::try_from(i).unwrap_or(motion::STAGGER_ROW_CAP),
            );
            // GLYPH-FIX — ●/○ (text-presentation BMP), not ☑/☐: U+2611 ☑ is
            // Emoji_Presentation=Yes, so it renders via the color-emoji font
            // (ignores tint, stalls first paint). Single-sourced from the typed
            // `mde_theme::Icon` table (StatusOk ● selected / StatusUnknown ○ not),
            // which resolves to those same text-presentation BMP codepoints.
            let sel_glyph = if selected {
                mde_theme::Icon::StatusOk.fallback_glyph()
            } else {
                mde_theme::Icon::StatusUnknown.fallback_glyph()
            };
            let mut row_el = row![
                button(text(sel_glyph).size(13)).on_press(Message::QueueToggleSelect(i)),
                text(format!("{marker}{label}"))
                    .size(13)
                    .colr(carbon(p.text, rv.alpha))
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
            row_el = row_el.push(
                button(text(mde_theme::Icon::Cancel.fallback_glyph()).size(12))
                    .on_press(Message::QueueRemove(i)),
            );
            // MOTION-FEEDBACK — the reveal's rise becomes a leading-padding offset
            // (translate→padding glue); under reduce-motion `slide_in` yields no
            // rise, so the row only crossfades into place.
            queue = queue.push(container(row_el).padding(cosmic::iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: rv.translate_y.max(0.0),
            }));
        }
        // MUSIC-RFX-9 — reserve the height of the rows below the window so the
        // scrollbar reflects the full queue length (the user can scroll all the way
        // down even though only the windowed rows are mounted).
        if win.virtualize && win.end < self.queue_songs.len() {
            let below = (self.queue_songs.len() - win.end) as f32 * QUEUE_ROW_PITCH;
            queue = queue.push(Space::new().height(Length::Fixed(below)));
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
                let status = if p.playing {
                    format!("{} playing", mde_theme::Icon::StatusOk.fallback_glyph())
                } else {
                    "paused".to_string()
                };
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
            // MUSIC-RFX-9 — id + on_scroll feed the row-window virtualization above.
            MaxiTab::Queue => scrollable(queue)
                .id(queue_scroll_id())
                .on_scroll(|vp| Message::QueueScrolled(vp.absolute_offset().y))
                .into(),
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

/// MOTION-FEEDBACK — adjust an iced `Color`'s luminance by `factor` (>1 lighter,
/// <1 darker), clamped, alpha preserved. The single spot for the feedback
/// tint-depth channel math (mirrors the `carbon` channel-math sanction, §4).
fn shade(c: cosmic::iced::Color, factor: f32) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: (c.r * factor).clamp(0.0, 1.0),
        g: (c.g * factor).clamp(0.0, 1.0),
        b: (c.b * factor).clamp(0.0, 1.0),
        a: c.a,
    }
}

/// MOTION-FEEDBACK — a transport / shuttle button carrying the shared motion
/// vocabulary: hover-**lift** + press-**depress** (the press fires on `down`, no
/// delay — iced re-styles the instant the status flips). The button widget can't
/// translate its own content (the iced 0.13 fork has no transform widget, and
/// `button::Style` carries no padding), so the shared
/// [`motion::button_feedback`] params drive the styleable proxies for the same
/// reads: a hover **lift** shows as a raised fill + a small drop-shadow
/// (elevation), and a press **depress** shows as a darkened fill with the
/// elevation removed (sinking). `feedback_tint_depth` sets the fill-tint depth.
/// **Under reduce-motion the lift/depress transform is suppressed**
/// ([`motion::button_feedback`] returns rest) — so the shadow/elevation cue is
/// dropped and only the (movement-free) tint state change reads (Q32).
fn transport_button<'a>(
    glyph: impl Into<String>,
    size: u16,
    msg: Message,
    reduce_motion: bool,
) -> Element<'a, Message> {
    let p = mde_theme::Palette::dark();
    let raised = carbon(p.raised, 1.0);
    let text_c = carbon(p.text, 1.0);
    button(text(glyph.into()).size(size).colr(text_c))
        .on_press(msg)
        .padding([6, 10])
        .sty(move |_t, status| {
            use cosmic::iced::widget::button::Status;
            let hovered = matches!(status, Status::Hovered | Status::Pressed);
            let pressed = matches!(status, Status::Pressed);
            // The shared motion params gate the *movement* reads (the lift
            // elevation / press sink); the tint depth is the movement-free cue.
            // `lifted` ⇒ the shared params rose the control (hover, motion on);
            // `Press` produces scale<1 with translate_y==0, so a lifted control is
            // never also depressed — gate the elevation on `lifted` alone.
            let lifted = motion::button_feedback(hovered, pressed, reduce_motion).translate_y < 0.0;
            let depth = motion::feedback_tint_depth(hovered, pressed);
            let fill = if pressed {
                // Press read: darken the raised fill toward the depressed tone.
                shade(raised, 1.0 - depth)
            } else if hovered {
                // Hover read: the full raised fill brightened by the tint depth.
                shade(raised, 1.0 + depth)
            } else {
                // Idle: the plain raised fill, so a transport glyph still reads as
                // a tappable control (the hover/press deltas ride on top of it).
                raised
            };
            // Lift → a small drop-shadow (elevation); press/idle → none. Dropped
            // under reduce-motion (no `lifted`), so the control never appears to
            // rise off the surface.
            let shadow = if lifted {
                cosmic::iced::Shadow {
                    color: carbon(p.background, 0.45),
                    offset: cosmic::iced::Vector::new(0.0, mde_theme::feedback::HOVER_LIFT_PX),
                    blur_radius: 4.0,
                }
            } else {
                cosmic::iced::Shadow::default()
            };
            cosmic::iced::widget::button::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                text_color: text_c,
                border: cosmic::iced::Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
                shadow,
                ..cosmic::iced::widget::button::Style::default()
            }
        })
        .into()
}

/// BEAUT-MUSIC — a primary-fill CTA (the accent-on-text button used by the
/// welcome card + empty-state actions) carrying the same shared motion
/// vocabulary as [`transport_button`]: hover-**lift** (a brightened accent fill +
/// a small drop-shadow) and press-**depress** (a darkened fill, elevation
/// removed), with the movement-free tint depth from [`motion::feedback_tint_depth`]
/// always reading. **Under reduce-motion the lift/depress transform is
/// suppressed** so only the (movement-free) tint state change reads (Q32).
fn primary_button<'a>(
    label: impl Into<String>,
    size: u16,
    msg: Message,
    reduce_motion: bool,
) -> Element<'a, Message> {
    let p = mde_theme::Palette::dark();
    let accent = carbon(p.accent, 1.0);
    // The WCAG-contrast text colour for the accent fill (white on the dark
    // indigo accent), derived from the token via the crate's `contrast_text`
    // helper — no hardcoded white literal (§4).
    let (tr, tg, tb) = color::contrast_text(color::accent_rgb());
    let on_accent = carbon(mde_theme::Rgba::rgb(tr, tg, tb), 1.0);
    button(text(label.into()).size(size).colr(on_accent))
        .on_press(msg)
        .padding([8, 18])
        .sty(move |_t, status| {
            use cosmic::iced::widget::button::Status;
            let hovered = matches!(status, Status::Hovered | Status::Pressed);
            let pressed = matches!(status, Status::Pressed);
            let lifted = motion::button_feedback(hovered, pressed, reduce_motion).translate_y < 0.0;
            let depth = motion::feedback_tint_depth(hovered, pressed);
            let fill = if pressed {
                shade(accent, 1.0 - depth)
            } else if hovered {
                shade(accent, 1.0 + depth)
            } else {
                accent
            };
            let shadow = if lifted {
                cosmic::iced::Shadow {
                    color: carbon(p.background, 0.45),
                    offset: cosmic::iced::Vector::new(0.0, mde_theme::feedback::HOVER_LIFT_PX),
                    blur_radius: 4.0,
                }
            } else {
                cosmic::iced::Shadow::default()
            };
            // A movement-free affordance ring (reduce-motion-safe): a subtle
            // accent-tint outline that strengthens on hover/press, so the CTA's
            // interactive state reads even when the lift transform is suppressed.
            let ring_alpha = if hovered { 0.55 } else { 0.0 };
            cosmic::iced::widget::button::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                text_color: on_accent,
                border: cosmic::iced::Border {
                    color: carbon(mde_theme::carbon::BLUE_40, ring_alpha),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                shadow,
                ..cosmic::iced::widget::button::Style::default()
            }
        })
        .into()
}

/// The stable widget id for the AIR-14 search field (so Cmd-F can focus it).
fn search_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-search")
}

/// POLISH-music-feedback (axis 5 — focus/a11y) — the search field's text-input
/// style: cosmic's standard input chrome for the rest / hover / error / disabled
/// states, with the **keyboard-focus** state drawing the shared 2px Carbon focus
/// ring ([`mde_theme::feedback::ControlFeedback::focus_ring`]) in place of the
/// default focus border. The search field is the surface's one keyboard-focusable
/// control, so wiring the shared ring here makes it speak the same focus
/// vocabulary as the rest of the shell — the ring width / offset / accent come
/// from the single-sourced `feedback` tokens (§4), never a local literal.
///
/// The field re-styles on the focus-status flip with no tween/tick, so the ring
/// is sampled at its arrived endpoint (a focus timestamp backdated past the focus
/// tween): it is *present* at full 2px whenever the field holds keyboard focus —
/// the a11y cue — and gone otherwise.
fn search_input_style(reduce_motion: bool) -> cosmic::theme::TextInput {
    use cosmic::widget::text_input::StyleSheet;
    // Delegate the non-focused states to cosmic's standard input appearance; only
    // the focused state is overridden, to carry the shared focus ring. (Each
    // closure builds its own `Default` base — the enum isn't `Copy` to capture.)
    cosmic::theme::TextInput::Custom {
        active: Box::new(|t| StyleSheet::active(t, &cosmic::theme::TextInput::Default)),
        error: Box::new(|t| StyleSheet::error(t, &cosmic::theme::TextInput::Default)),
        hovered: Box::new(|t| StyleSheet::hovered(t, &cosmic::theme::TextInput::Default)),
        focused: Box::new(move |t| {
            let mut a = StyleSheet::focused(t, &cosmic::theme::TextInput::Default);
            let now = std::time::Instant::now();
            let since = now
                .checked_sub(mde_theme::motion::Motion::focus().duration * 2)
                .unwrap_or(now);
            let ring = mde_theme::feedback::ControlFeedback::new()
                .focused(true, since)
                .focus_ring(now, reduce_motion);
            a.border_width = ring.width;
            a.border_offset = Some(ring.offset);
            a.border_color = carbon(mde_theme::Palette::dark().accent, ring.alpha);
            a
        }),
        disabled: Box::new(|t| StyleSheet::disabled(t, &cosmic::theme::TextInput::Default)),
    }
}

/// AIR-11.c.2 — stable id for the library card grid's scrollable, so the
/// scroll position can be saved on scroll + restored on category re-entry.
fn grid_scroll_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-grid")
}

/// MUSIC-RFX-9 — stable id for the maxi Queue list's scrollable, so its scroll
/// offset feeds the row-window virtualization (`on_scroll` → `QueueScrolled`).
fn queue_scroll_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("mde-music-queue")
}

/// MUSIC-RFX-9 — the fixed per-row height of a maxi Queue row (px). A row's
/// tallest child is a `button(text(...).size(13))`: in the vendored libcosmic iced
/// that's `1.4 * 13` (default `LineHeight::Relative(1.4)`) `+ 5 + 5` (button
/// `DEFAULT_PADDING` top/bottom) = 28.2 px, and the enclosing `column!().spacing(4)`
/// adds a 4 px inter-row gap → a 32.2 px pitch. It must match the rendered height
/// so the off-window spacers reserve exactly what the mounted rows would occupy
/// (a too-small pitch would drift the window off the viewport over a long queue).
const QUEUE_ROW_PITCH: f32 = 32.2;

/// MUSIC-RFX-9 — how many rows to keep mounted around the viewport. ~24 rows
/// covers a tall maxi window plus overscan above/below so a fast scroll never
/// reveals an un-built gap before the next `QueueScrolled` frame.
const QUEUE_WINDOW_ROWS: usize = 24;

/// MUSIC-RFX-9 — only virtualize once the queue is taller than the window (below
/// that a full render is cheaper than the spacer bookkeeping), mirroring the
/// library grid's `total_rows > WINDOW_ROWS` gate (RESPONSIVE-9).
const QUEUE_VIRTUALIZE_THRESHOLD: usize = QUEUE_WINDOW_ROWS;

/// MUSIC-RFX-9 — the half-open `[start, end)` row range the maxi Queue should
/// mount for a given total row count + scroll offset, plus whether to virtualize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueueWindow {
    /// First row index to mount (0 when not virtualizing).
    start: usize,
    /// One past the last row to mount (`total` when not virtualizing).
    end: usize,
    /// `false` for a short queue rendered in full (no spacers).
    virtualize: bool,
}

impl QueueWindow {
    /// Resolve which queue rows to mount. Below the threshold the whole queue
    /// renders (`virtualize=false`, `[0, total)`). Above it, a `QUEUE_WINDOW_ROWS`
    /// band centred on the scroll offset is mounted, started two rows early as
    /// overscan and clamped to the queue so a stale (too-large) offset never
    /// leaves a blank window. Pure (no widgets) so it's unit-tested.
    fn resolve(total: usize, offset_y: f32) -> Self {
        if total <= QUEUE_VIRTUALIZE_THRESHOLD {
            return Self {
                start: 0,
                end: total,
                virtualize: false,
            };
        }
        // The first fully-scrolled-past row, minus a two-row overscan lead.
        let lead = ((offset_y.max(0.0) / QUEUE_ROW_PITCH).floor() as usize).saturating_sub(2);
        // Clamp the window so it always shows a full band even when the offset is
        // stale past the (now shorter) end of the queue.
        let max_start = total.saturating_sub(QUEUE_WINDOW_ROWS);
        let start = lead.min(max_start);
        let end = (start + QUEUE_WINDOW_ROWS).min(total);
        Self {
            start,
            end,
            virtualize: true,
        }
    }
}

/// BEAUT-MUSIC — render one Carbon skeleton placeholder: a rounded, breathing
/// rectangle. Geometry (width / height / corner) is the shared
/// [`mde_theme::SkeletonBlock`] token shape (`width == None` ⇒ fill the available
/// width); `fill` is the resolved [`mde_theme::SkeletonShimmer::fill`] — the
/// palette `text` token at the live shimmer alpha — so every skeleton on the
/// surface breathes on the one shared convention and the corner radius is a
/// [`mde_theme::Radii`] token, never a re-derived literal (§4 / §6: consume the
/// primitive, don't reimplement it).
fn skeleton_block(
    block: mde_theme::SkeletonBlock,
    fill: mde_theme::Rgba,
) -> Element<'static, Message> {
    let width = block
        .width
        .map_or(Length::Fill, |w| Length::Fixed(f32::from(w)));
    let color = carbon(fill, fill.a);
    let radius = f32::from(block.radius);
    container(
        Space::new()
            .width(width)
            .height(Length::Fixed(f32::from(block.height))),
    )
    .style(move |_| cosmic::iced::widget::container::Style {
        background: Some(color.into()),
        border: cosmic::iced::Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        },
        ..Default::default()
    })
    .into()
}

/// MUSIC-RESPONSIVE-6 / BEAUT-MUSIC — a grid of greyed Carbon skeleton tiles
/// shown while a category loads, matching the real card geometry (the
/// single-sourced Carbon grid metrics: art tile + gutter + card width) so
/// navigation paints structure within one frame instead of a blank pane. The
/// tiles **breathe** via `fill` — the shared [`mde_theme::SkeletonShimmer::fill`]
/// — so a slow load reads as active; under reduce-motion that fill is the static
/// mid grey (no movement). `cols` mirrors the real grid's width-adaptive column
/// count.
fn skeleton_grid(cols: usize, fill: mde_theme::Rgba) -> Element<'static, Message> {
    let radii = mde_theme::Radii::defaults();
    // MUSIC-RFX-10 — the skeleton tile mirrors the real card geometry from the
    // single-sourced Carbon grid metrics (art tile + gutter + card width), so the
    // loading placeholder lines up pixel-for-pixel with the cards it precedes.
    let gm = GridMetrics::carbon_dense();
    let tile = move || -> Element<'static, Message> {
        // The art tile fills the card width; `art_height` is an integer-valued
        // metric (`f32::from(u16)`), so the down-cast is exact. The label below is
        // a standard skeleton text line (`sm` corner, text-line height).
        let art = mde_theme::SkeletonBlock {
            width: None,
            height: gm.art_height as u16,
            radius: radii.sm,
        };
        column![
            skeleton_block(art, fill),
            skeleton_block(mde_theme::SkeletonBlock::line(Some(110), radii), fill),
        ]
        .spacing(gm.gap)
        .width(Length::Fixed(gm.card_width))
        .into()
    };
    // Two rows of placeholders — enough to fill the typical viewport.
    let mut grid = column![].spacing(gm.gap);
    for _ in 0..2 {
        let mut r = row![].spacing(gm.gap);
        for _ in 0..cols {
            r = r.push(tile());
        }
        grid = grid.push(r);
    }
    grid.into()
}

/// BEAUT-MUSIC — a single breathing Carbon skeleton bar of `w`×`h` px, the
/// building block for the Home dashboard's loading placeholder (hero counts /
/// server line / chips). A pinned [`mde_theme::SkeletonBlock`] at the `sm` Carbon
/// corner, painted with the shared [`mde_theme::SkeletonShimmer::fill`]; under
/// reduce-motion that fill is the static mid grey.
fn skeleton_bar(w: u16, h: u16, fill: mde_theme::Rgba) -> Element<'static, Message> {
    let block = mde_theme::SkeletonBlock {
        width: Some(w),
        height: h,
        radius: mde_theme::Radii::defaults().sm,
    };
    skeleton_block(block, fill)
}

/// POLISH-music-errorretry — a breathing list-loading placeholder: `rows` skeleton
/// track rows (a short index bar + a fill-width title bar), reusing the shared
/// [`mde_theme::SkeletonBlock`] / [`mde_theme::SkeletonShimmer`] convention the
/// grid and Home dashboard already use, for the album / playlist / search loading
/// states. `fill` is the resolved shimmer tint (static under reduce-motion).
/// Placeholder bar widths follow the established skeleton geometry convention (the
/// §4 lint scopes colours + motion, not skeleton px shapes).
fn loading_list_skeleton(
    fill: mde_theme::Rgba,
    m: ListMetrics,
    rows: usize,
) -> Element<'static, Message> {
    let radii = mde_theme::Radii::defaults();
    let mut col = column![].spacing(m.row_gap);
    for _ in 0..rows {
        col = col.push(
            row![
                skeleton_block(mde_theme::SkeletonBlock::line(Some(24), radii), fill),
                skeleton_block(mde_theme::SkeletonBlock::line(None, radii), fill),
            ]
            .spacing(m.col_gap),
        );
    }
    col.into()
}

/// POLISH-music-errorretry — the album detail page's loading placeholder: two
/// header skeleton bars (title · artist) over the shared track-list skeleton, the
/// Carbon convention shown while the album fetches. Split out of `album_page` so
/// the page renderer stays within the readable line budget.
fn album_loading_skeleton(fill: mde_theme::Rgba) -> Element<'static, Message> {
    let m = ListMetrics::carbon_dense();
    let radii = mde_theme::Radii::defaults();
    column![
        skeleton_block(mde_theme::SkeletonBlock::line(Some(220), radii), fill),
        skeleton_block(mde_theme::SkeletonBlock::line(Some(140), radii), fill),
        Space::new().height(Length::Fixed(f32::from(m.header_gap))),
        loading_list_skeleton(fill, m, 8),
    ]
    .spacing(m.row_gap)
    .padding(m.pad)
    .into()
}

/// POLISH-music-errorretry — classify a daemon/library load-error string onto the
/// shared [`mde_theme::LoadState`], so every failed surface renders the one Carbon
/// async-state vocabulary (icon + label + tone) rather than a bare red line. A
/// connectivity-class failure (daemon/mesh unreachable) is the recoverable
/// [`mde_theme::LoadState::Offline`] — a cached/empty view waiting to reconnect;
/// anything else is a terminal [`mde_theme::LoadState::Failed`]. Both report
/// `can_retry()`, so both surface the Retry affordance. Pure → unit-tested.
fn load_state_for_error(err: &str) -> mde_theme::LoadState {
    let e = err.to_ascii_lowercase();
    let connectivity = e.contains("not responding")
        || e.contains("unreachable")
        || e.contains("connection refused")
        || e.contains("no connection")
        || e.contains("no route")
        || e.contains("timed out")
        || e.contains("timeout")
        || e.contains("offline");
    if connectivity {
        mde_theme::LoadState::Offline
    } else {
        mde_theme::LoadState::Failed
    }
}

/// POLISH-music-errorretry — the Carbon support colour token for a state's
/// [`mde_theme::StateTone`]. Tone is the *secondary* cue layered under the icon +
/// label (the a11y contract: never colour alone); every arm reads a `Palette`
/// token (§4).
const fn tone_token(p: &mde_theme::Palette, tone: mde_theme::StateTone) -> mde_theme::Rgba {
    match tone {
        mde_theme::StateTone::Neutral => p.text_muted,
        mde_theme::StateTone::Info => p.accent,
        mde_theme::StateTone::Warning => p.warning,
        mde_theme::StateTone::Danger => p.danger,
        mde_theme::StateTone::Success => p.success,
    }
}

/// POLISH-music-errorretry — the shared failed/offline state block used by the
/// grid, album, and search surfaces: the [`mde_theme::LoadState`] icon + label
/// (the non-motion a11y pair) in the tone colour, the raw error `detail` beneath,
/// and a **Retry** button shown only when [`mde_theme::LoadState::can_retry`] AND a
/// retry message is wired — so it is never a dead control (§7). Centred + padded,
/// mirroring the empty-state composition. Every colour is an `mde-theme` token (§4).
fn load_error_block(
    state: mde_theme::LoadState,
    detail: &str,
    m: ListMetrics,
    retry: Option<Message>,
) -> Element<'static, Message> {
    let p = mde_theme::Palette::dark();
    let tone = carbon(tone_token(&p, state.tone()), 1.0);
    let mut col = column![
        text(state.icon().to_string()).size(m.heading).colr(tone),
        text(state.label()).size(m.body).colr(tone),
        text(detail.to_string())
            .size(m.body)
            .colr(carbon(p.text_muted, 1.0)),
    ]
    .spacing(m.row_gap)
    .align_x(cosmic::iced::Alignment::Center);
    if let Some(msg) = retry {
        if state.can_retry() {
            col = col.push(button(text("Retry").size(m.body)).on_press(msg));
        }
    }
    container(col)
        .width(Length::Fill)
        .padding(m.pad)
        .center_x(Length::Fill)
        .into()
}

/// BEAUT-MUSIC — a tasteful Carbon empty state: a muted hero glyph over a
/// heading + a one-line body, centered, instead of a bare one-liner. Mirrors the
/// shared `mde_theme::EmptyState` data shape (icon · heading · body) — kept as a
/// local renderer since mde-music builds `cosmic::Theme` widgets directly and the
/// `mde-workbench` widget builder is shell-side. Every colour is an `mde-theme`
/// token (§4).
fn empty_state(heading: &str, body: &str) -> Element<'static, Message> {
    let p = mde_theme::Palette::dark();
    let muted = carbon(p.text_muted, 1.0);
    container(
        column![
            text("\u{266A}").size(32).colr(muted),
            Space::new().height(Length::Fixed(12.0)),
            text(heading.to_string()).size(16).colr(carbon(p.text, 1.0)),
            Space::new().height(Length::Fixed(6.0)),
            text(body.to_string()).size(13).colr(muted),
        ]
        .spacing(0)
        .align_x(cosmic::iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(48)
    .center_x(Length::Fill)
    .into()
}

/// POLISH-music-playlistcreate / POLISH-music-emptycopy — the single source of
/// empty-state copy for every page that renders through the grid path. Returns
/// `(heading, body, show_create_form)`.
///
/// The copy is honest per route. By the time an empty state renders, the load
/// has already succeeded — a daemon that is down or unreachable renders the
/// Offline error block instead (see `load_error_block`) — so the old generic
/// "Start mde-musicd" hint was misleading on every page: a sub-page the operator
/// navigated *into* (an artist, a genre, a channel) is itself proof the daemon
/// is up. Each route therefore names its own empty container, never the daemon.
///
/// The Playlists landing is the one empty state that doubles as an entry point:
/// a zero-playlist operator has no other way to create their first (the
/// add-to-playlist sheet only points back here), so its copy pairs with the
/// inline create form (the third field).
const fn empty_state_for(route: &Route) -> (&'static str, &'static str, bool) {
    match route {
        Route::Category(HubCard::Playlists) => (
            "No playlists yet",
            "Name your first playlist below to get started.",
            true,
        ),
        Route::Category(HubCard::Albums) => (
            "No albums in your library",
            "Albums you add appear here.",
            false,
        ),
        Route::Category(HubCard::Artists) => (
            "No artists in your library",
            "Artists you add appear here.",
            false,
        ),
        Route::Category(HubCard::Genres) => (
            "No genres in your library",
            "Genres appear here as your tracks are tagged.",
            false,
        ),
        Route::Category(HubCard::Recents) => (
            "Nothing played recently",
            "Tracks you play appear here.",
            false,
        ),
        Route::Category(HubCard::Podcasts) => (
            "No podcast subscriptions",
            "Channels you subscribe to appear here.",
            false,
        ),
        Route::Category(HubCard::Radio) => {
            ("No radio stations", "Stations you add appear here.", false)
        }
        // Sub-pages: reached by navigating in, so the daemon is plainly up —
        // name the empty container, never "start the daemon".
        Route::Artist(..) => (
            "No albums for this artist",
            "This artist has nothing in your library yet.",
            false,
        ),
        Route::Genre(..) => (
            "No albums in this genre",
            "Nothing in your library is tagged with this genre.",
            false,
        ),
        Route::Podcast(..) => (
            "No episodes yet",
            "This channel has no episodes to show.",
            false,
        ),
        // Hub/Album/Playlist render their own pages and Search uses its own
        // sheet, so they never reach here; a neutral fallback keeps the match
        // total without a misleading daemon hint.
        Route::Hub | Route::Album(..) | Route::Playlist(..) | Route::Search(..) => {
            ("Nothing here yet", "There's nothing to show here.", false)
        }
    }
}

/// Render one search section: a heading + a clickable row per item. An
/// empty section renders nothing. `on_click` maps an item to its message.
fn result_section<'a>(
    title: &'a str,
    items: &'a [LibraryItem],
    m: ListMetrics,
    on_click: impl Fn(&LibraryItem) -> Message,
) -> Element<'a, Message> {
    let mut col = column![].spacing(m.row_gap);
    if items.is_empty() {
        return col.into();
    }
    col = col.push(text(title).size(m.body));
    for item in items {
        col = col.push(button(text(item.label.clone()).size(m.body)).on_press(on_click(item)));
    }
    col = col.push(Space::new().height(Length::Fixed(f32::from(m.header_gap))));
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

#[cfg(test)]
mod empty_state_tests {
    use super::{empty_state_for, HubCard, Route};

    #[test]
    fn playlists_empty_state_offers_create() {
        // POLISH-music-playlistcreate — a zero-playlist operator must be able to
        // create their first from the Playlists empty state; the third tuple
        // field gates the inline create-playlist form.
        let (heading, _body, show_create) = empty_state_for(&Route::Category(HubCard::Playlists));
        assert!(
            show_create,
            "Playlists empty state must show the create form"
        );
        assert_eq!(heading, "No playlists yet");
    }

    #[test]
    fn other_categories_have_no_create_form() {
        // Albums/Artists/etc. have no inline create affordance — only their
        // honest per-route empty copy (the create form is Playlists-only).
        for card in [HubCard::Albums, HubCard::Artists, HubCard::Genres] {
            let (_h, _b, show_create) = empty_state_for(&Route::Category(card));
            assert!(!show_create, "{card:?} must not show a create form");
        }
    }

    #[test]
    fn no_empty_state_tells_the_operator_to_start_the_daemon() {
        // POLISH-music-emptycopy — reaching an empty state means the load already
        // succeeded; a daemon that is down renders the Offline error block, not
        // this. So no empty-state copy may name the daemon — that hint is honest
        // only in the error path, never here.
        let routes = [
            Route::Category(HubCard::Albums),
            Route::Category(HubCard::Artists),
            Route::Category(HubCard::Playlists),
            Route::Category(HubCard::Recents),
            Route::Category(HubCard::Genres),
            Route::Category(HubCard::Podcasts),
            Route::Category(HubCard::Radio),
            Route::Artist("1".into(), "Air".into()),
            Route::Genre("Jazz".into()),
            Route::Podcast("p".into(), "Pod".into()),
        ];
        for r in routes {
            let (heading, body, _) = empty_state_for(&r);
            assert!(
                !heading.is_empty() && !body.is_empty(),
                "{r:?} needs non-empty copy"
            );
            let copy = format!("{heading} {body}").to_lowercase();
            assert!(
                !copy.contains("mde-musicd"),
                "{r:?} empty copy must not name the daemon: {copy:?}"
            );
        }
    }

    #[test]
    fn each_top_level_category_names_its_own_emptiness() {
        // POLISH-music-emptycopy — every library category gets distinct copy that
        // names what is empty (an empty Albums page reads "No albums…"), not one
        // reused generic line.
        let cases = [
            (HubCard::Albums, "albums"),
            (HubCard::Artists, "artists"),
            (HubCard::Genres, "genres"),
            (HubCard::Recents, "recently"),
            (HubCard::Podcasts, "podcast"),
            (HubCard::Radio, "radio"),
        ];
        for (card, needle) in cases {
            let (heading, _b, _c) = empty_state_for(&Route::Category(card));
            assert!(
                heading.to_lowercase().contains(needle),
                "{card:?} heading {heading:?} should name {needle:?}"
            );
        }
    }

    #[test]
    fn sub_pages_get_their_own_copy_distinct_from_the_generic_fallback() {
        // POLISH-music-emptycopy — a sub-page the operator navigated into must
        // describe its own empty container, not fall through to the neutral
        // "Nothing here yet" fallback (reserved for routes that never render here).
        for r in [
            Route::Artist("1".into(), "Air".into()),
            Route::Genre("Jazz".into()),
            Route::Podcast("p".into(), "Pod".into()),
        ] {
            let (heading, _b, show_create) = empty_state_for(&r);
            assert!(!show_create, "{r:?} must not show a create form");
            assert_ne!(heading, "Nothing here yet", "{r:?} needs its own heading");
        }
    }
}

#[cfg(test)]
mod load_error_tests {
    use super::load_state_for_error;
    use mde_theme::LoadState;

    #[test]
    fn connectivity_errors_classify_as_offline() {
        // POLISH-music-errorretry — the daemon-not-warm / unreachable family is
        // the recoverable Offline state (a cached/empty view waiting to reconnect),
        // not a terminal Failed.
        for msg in [
            "daemon not responding",
            "Connection refused (os error 111)",
            "host unreachable",
            "request timed out",
            "read timeout",
            "no route to host",
            "backend OFFLINE",
        ] {
            assert_eq!(
                load_state_for_error(msg),
                LoadState::Offline,
                "{msg:?} should classify as Offline"
            );
        }
    }

    #[test]
    fn other_errors_classify_as_failed() {
        // A non-connectivity error is a terminal Failed (the request itself was
        // rejected) — distinct from a transport outage.
        for msg in [
            "bad request: unknown verb",
            "deserialize error: missing field `id`",
            "permission denied",
        ] {
            assert_eq!(
                load_state_for_error(msg),
                LoadState::Failed,
                "{msg:?} should classify as Failed"
            );
        }
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            load_state_for_error("DAEMON NOT RESPONDING"),
            LoadState::Offline
        );
        assert_eq!(
            load_state_for_error("TimeOut while reading"),
            LoadState::Offline
        );
    }

    #[test]
    fn every_classified_error_state_offers_retry() {
        // The contract behind the shared error block: the error→LoadState logic
        // only ever yields retryable states (Offline / Failed), so a real load
        // failure always surfaces the Retry affordance — never a dead-end screen.
        for msg in [
            "daemon not responding",
            "host unreachable",
            "weird internal error",
            "",
        ] {
            let s = load_state_for_error(msg);
            assert!(
                s.can_retry(),
                "{msg:?} → {s:?} must offer Retry (is_error or recoverable)"
            );
        }
    }
}

#[cfg(test)]
mod launch_view_tests {
    use super::LaunchView;

    #[test]
    fn creds_present_auto_browses() {
        // MEDIA-8 — a fresh node whose airsonic-creds.json was auto-written by
        // mackesd's music_autoconfig worker BROWSES on launch (no first-run gate).
        assert_eq!(LaunchView::from_creds_present(true), LaunchView::Browse);
    }

    #[test]
    fn no_creds_falls_back_to_first_run() {
        // A genuinely unconfigured node (no creds at all) still lands on the
        // first-run connect form — the override-only fallback.
        assert_eq!(LaunchView::from_creds_present(false), LaunchView::FirstRun);
    }
}

#[cfg(test)]
mod queue_window_tests {
    use super::{QueueWindow, QUEUE_ROW_PITCH, QUEUE_VIRTUALIZE_THRESHOLD, QUEUE_WINDOW_ROWS};

    #[test]
    fn short_queue_renders_in_full() {
        // MUSIC-RFX-9 — at/below the threshold the whole queue mounts (no spacers,
        // no virtualization), regardless of the scroll offset.
        for total in [0_usize, 1, QUEUE_VIRTUALIZE_THRESHOLD] {
            let w = QueueWindow::resolve(total, 0.0);
            assert!(!w.virtualize, "total={total} should render full");
            assert_eq!(w.start, 0);
            assert_eq!(w.end, total);
        }
    }

    #[test]
    fn long_queue_windows_around_the_top_when_unscrolled() {
        // A large queue at offset 0 mounts exactly the first WINDOW_ROWS band.
        let total = 5000;
        let w = QueueWindow::resolve(total, 0.0);
        assert!(w.virtualize);
        assert_eq!(w.start, 0);
        assert_eq!(w.end, QUEUE_WINDOW_ROWS);
        // The window is strictly smaller than the queue — the win for big queues.
        assert!(w.end - w.start < total);
    }

    #[test]
    fn window_follows_the_scroll_offset_with_overscan() {
        // Scrolled 100 rows down, the window leads two rows early (overscan) and
        // spans WINDOW_ROWS, so the on-screen rows are always built ahead of the
        // viewport.
        let total = 5000;
        let offset = 100.0 * QUEUE_ROW_PITCH;
        let w = QueueWindow::resolve(total, offset);
        assert!(w.virtualize);
        assert_eq!(w.start, 98, "two-row overscan lead");
        assert_eq!(w.end, 98 + QUEUE_WINDOW_ROWS);
    }

    #[test]
    fn stale_offset_past_the_end_still_shows_a_full_band() {
        // After a remove shrinks the queue, a now-too-large offset must not leave a
        // blank window: the start clamps so a full WINDOW_ROWS band still mounts at
        // the tail (the regression a naive floor() would cause — start past total).
        let total = 30;
        let offset = 10_000.0 * QUEUE_ROW_PITCH; // far past the (now short) queue
        let w = QueueWindow::resolve(total, offset);
        assert!(w.virtualize);
        assert_eq!(w.end, total, "window ends at the real tail");
        assert_eq!(
            w.end - w.start,
            QUEUE_WINDOW_ROWS,
            "a full band is still shown, not a blank window"
        );
    }

    #[test]
    fn negative_offset_clamps_to_the_top() {
        // A bounce/overscroll can momentarily report a negative offset; it must
        // resolve to the top window, never an underflowed start.
        let w = QueueWindow::resolve(5000, -500.0);
        assert_eq!(w.start, 0);
        assert_eq!(w.end, QUEUE_WINDOW_ROWS);
    }
}

#[cfg(test)]
mod transport_outcome_tests {
    use super::{resolve_transport_outcome, NowState, TransportAction, TransportOutcome};

    fn paused_state() -> NowState {
        NowState {
            playing: false,
            active: true,
            song_id: "s7".to_string(),
            position_ms: 12_000,
            volume: 0.5,
            ..NowState::default()
        }
    }

    #[test]
    fn success_reconciles_when_action_is_current() {
        // MOTION-NET-4 — a successful transport whose action is the pending one
        // reconciles from a fresh fetch (clearing pending/failed).
        let action = TransportAction::PlayPause { was_playing: false };
        let pending = Some((action, paused_state()));
        assert_eq!(
            resolve_transport_outcome(pending, action, Ok(())),
            TransportOutcome::Reconcile
        );
    }

    #[test]
    fn failure_reverts_to_the_pre_action_snapshot() {
        // A failed action reverts to the EXACT pre-action state (no blank footer)
        // and arms a retry for the same action.
        let action = TransportAction::SkipNext;
        let snap = paused_state();
        let pending = Some((action, snap.clone()));
        match resolve_transport_outcome(pending, action, Err(())) {
            TransportOutcome::Revert {
                snapshot,
                action: reverted,
            } => {
                assert_eq!(snapshot, snap, "revert restores the pre-action view");
                assert_eq!(reverted, action, "the retry re-issues the same action");
            }
            other => panic!("expected Revert, got {other:?}"),
        }
    }

    #[test]
    fn seek_revert_restores_the_prior_position() {
        // A failed seek must put the playhead back where it was, not at the
        // dragged-to position (the optimistic jump is undone).
        let action = TransportAction::Seek {
            position_ms: 90_000,
        };
        let snap = paused_state(); // position_ms == 12_000
        let pending = Some((action, snap.clone()));
        match resolve_transport_outcome(pending, action, Err(())) {
            TransportOutcome::Revert { snapshot, .. } => {
                assert_eq!(snapshot.position_ms, 12_000);
            }
            other => panic!("expected Revert, got {other:?}"),
        }
    }

    #[test]
    fn stale_completion_is_ignored() {
        // A completion whose action no longer matches the pending slot (a newer
        // action raced ahead) is ignored — never reverting to an outdated snapshot
        // or blanking the footer the newer action now owns.
        let stale = TransportAction::PlayPause { was_playing: true };
        let current = TransportAction::SkipNext;
        let pending = Some((current, paused_state()));
        // Both a stale success and a stale failure are ignored.
        assert_eq!(
            resolve_transport_outcome(pending.clone(), stale, Ok(())),
            TransportOutcome::Ignore
        );
        assert_eq!(
            resolve_transport_outcome(pending, stale, Err(())),
            TransportOutcome::Ignore
        );
    }

    #[test]
    fn completion_with_no_pending_is_ignored() {
        // A completion arriving when nothing is pending (e.g. after a reconcile
        // already cleared it) is a no-op.
        let action = TransportAction::SkipPrev;
        assert_eq!(
            resolve_transport_outcome(None, action, Ok(())),
            TransportOutcome::Ignore
        );
        assert_eq!(
            resolve_transport_outcome(None, action, Err(())),
            TransportOutcome::Ignore
        );
    }

    #[test]
    fn failed_label_is_action_specific() {
        // Each action surfaces its own retry-banner copy.
        assert!(TransportAction::SkipNext.failed_label().contains("next"));
        assert!(TransportAction::SkipPrev
            .failed_label()
            .contains("previous"));
        assert!(TransportAction::SetVolume { level: 0.5 }
            .failed_label()
            .contains("volume"));
        assert!(TransportAction::Seek { position_ms: 0 }
            .failed_label()
            .contains("seek"));
        assert!(TransportAction::PlayPause { was_playing: true }
            .failed_label()
            .contains("playback"));
    }
}

#[cfg(test)]
mod icon_glyph_tests {
    // POLISH-music-icons — the surface routes its inline glyphs through the typed
    // `mde_theme::Icon` table (single source, §6) instead of scattering literals.
    // These contracts pin the codepoints the music layout assumes: if the shared
    // table ever remaps one of these, the now-playing/queue rows change glyph in
    // lock-step with the source of truth — and the second test guards the
    // GLYPH-FIX hazard (a status dot must stay a text-presentation BMP glyph, not
    // an emoji-presentation one the colour-emoji font would claim, ignoring tint).
    use mde_theme::Icon;

    #[test]
    fn routed_icons_resolve_to_the_expected_text_glyphs() {
        assert_eq!(Icon::Audio.fallback_glyph(), "\u{266A}"); // ♪ now-playing
        assert_eq!(Icon::StatusOk.fallback_glyph(), "\u{25CF}"); // ● filled dot
        assert_eq!(Icon::StatusUnknown.fallback_glyph(), "\u{25CB}"); // ○ idle dot
        assert_eq!(Icon::ChevronRight.fallback_glyph(), "\u{203A}"); // › breadcrumb
        assert_eq!(Icon::Confirm.fallback_glyph(), "\u{2713}"); // ✓ commit
        assert_eq!(Icon::Cancel.fallback_glyph(), "\u{00D7}"); // × cancel/remove
    }

    #[test]
    fn status_dot_glyphs_stay_text_presentation_bmp() {
        for icon in [Icon::StatusOk, Icon::StatusUnknown] {
            let g = icon.fallback_glyph();
            let ch = g.chars().next().expect("glyph is non-empty");
            assert_eq!(g.chars().count(), 1, "{icon:?} must be a single char");
            assert!(
                (ch as u32) < 0x1_0000,
                "{icon:?} must stay in the BMP (text presentation)"
            );
        }
    }
}
