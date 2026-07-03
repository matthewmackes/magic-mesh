//! The render-agnostic view-model + controller for the media surface (MEDIA-8).
//!
//! This module holds **no egui types** — only the state the UI renders and the
//! controller that drives [`mde_media_core`]. It is deliberately thin (§6 glue): the
//! authoritative state lives in the core — the [`Player`] state machine (transport +
//! the MEDIA-6 advanced controls), the [`Library`] index (MEDIA-7 browse folds), the
//! [`Playlist`] queue (MEDIA-6), and the `ResumeState` history (MEDIA-7). The
//! controller just translates a UI intent ([`TransportAction`]) into the matching
//! core method and surfaces the result, and exposes the core's data (the browse fold,
//! the source list, the now-playing view) for the views to render.
//!
//! Because it never touches egui or a GPU — and drives the core's `FakeMpv` seam in
//! the default build — the whole controller is unit-tested below (the transport glue
//! against the real player, the browse/source/OSD folds as pure functions).

use mde_jellyfin::{
    build_playback_decision, direct_play_url, BaseItemDto, CacheEntry, CacheRequest,
    ClientCapabilities, HttpTransport, ItemsQuery, JellyfinClient, JellyfinError, MediaSourceInfo,
    OfflineCache, PlaybackDecision, PlaybackMethod, PlaybackReport, ServerAuth, ServerConfig,
    ServerStore, StreamMediaType,
};
use mde_media_core::{
    classify_url, AbLoop, BrowseQuery, Library, LibraryItem, MediaEngine, MediaKind,
    MpvCapabilities, PlaybackControls, Player, PlayerEvent, PlayerState, Playlist, PlaylistItem,
    RepeatMode, ScreenshotMode, SortKey, Track, TrackKind, TrackSelect, TrackSelection, UrlKind,
    YtDlpError, YtDlpResolver,
};

/// The seed used when the operator toggles shuffle on.
///
/// Fixed (not wall-clock) so the shuffle order — and therefore the tests — stay
/// deterministic, matching [`Playlist::shuffle`]'s "deterministic so it's testable"
/// contract.
pub const SHUFFLE_SEED: u64 = 0x5EED_5EED_5EED_5EED;

/// How many seconds of pointer inactivity hide the auto-hiding media OSD (design
/// Q32). Named here rather than scattered so the dwell lives in one place.
pub const OSD_HIDE_SECS: f64 = 3.0;

/// The four top-level views of the media app (design Q31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaTab {
    /// The Sources list — indexed local roots (+ where mesh / Jellyfin sources land).
    #[default]
    Sources,
    /// The Library browse — the [`Library::browse`] fold (search + filter + sort).
    Library,
    /// The Player view — the video stage, transport, and the MEDIA-6 controls.
    Player,
    /// The Queue view — the [`Playlist`] with repeat / shuffle.
    Queue,
}

impl MediaTab {
    /// The tab's display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Library => "Library",
            Self::Player => "Player",
            Self::Queue => "Queue",
        }
    }

    /// The tabs in bar order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Sources, Self::Library, Self::Player, Self::Queue]
    }
}

/// The render-agnostic UI state that is *not* owned by the core.
///
/// Holds which view is active, the editable browse query, the mini-player /
/// fullscreen toggles, the OSD dwell, and the transient status line. Everything else
/// the views need comes from the core through the [`MediaController`].
#[derive(Debug, Clone, Default)]
pub struct UiState {
    /// The active top-level view.
    pub tab: MediaTab,
    /// The live browse query the Library view edits and [`MediaController::visible_items`]
    /// runs against the [`Library`].
    pub query: BrowseQuery,
    /// The text buffer behind the search field (mirrors `query.search`).
    pub search_input: String,
    /// The text buffer behind the "index a folder" field on Sources.
    pub folder_input: String,
    /// The text buffer behind the "Open URL" (network stream / web video) field on
    /// Sources (MEDIA-12).
    pub url_input: String,
    /// The text buffer behind the Jellyfin "server name" field on Sources.
    pub jellyfin_name_input: String,
    /// The text buffer behind the Jellyfin "server URL" field on Sources.
    pub jellyfin_url_input: String,
    /// Whether the `PiP` mini-player (design Q31/Q32) is shown.
    pub pip: bool,
    /// Whether the surface is in immersive fullscreen (design Q32).
    pub fullscreen: bool,
    /// Seconds of pointer inactivity, accumulated by the app each frame and read by
    /// [`osd_should_show`] to auto-hide the media OSD.
    pub osd_idle_secs: f64,
    /// The pending A-marker of an A-B loop awaiting its B (design Q12). `None` when no
    /// A-B loop is being defined.
    pub ab_pending: Option<f64>,
    /// A transient status / error line (the last refused transport, a snapshot
    /// confirmation, an index result). Rendered honestly, never swallowed (§7).
    pub status: Option<String>,
}

/// A row in the Sources list — one indexed local root and how much it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    /// A short display name (the folder's own name).
    pub label: String,
    /// The root path (the [`Library`] key prefix + what re-index walks).
    pub path: String,
    /// How many indexed items live under this root.
    pub item_count: usize,
}

/// A configured Jellyfin server as a Sources row — its display name, base URL,
/// whether it is signed in, and its user profiles (MEDIA-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JellyfinSourceRow {
    /// The stable local id (the [`ServerStore`] key).
    pub id: String,
    /// The server's display name.
    pub label: String,
    /// The base URL.
    pub base_url: String,
    /// Whether a saved token is present (signed in).
    pub signed_in: bool,
    /// Whether this is the currently selected server.
    pub selected: bool,
    /// The active profile's display name, if any (MEDIA-11).
    pub active_profile: Option<String>,
    /// The user profiles configured on this server, for the switcher (MEDIA-11).
    pub profiles: Vec<JellyfinProfileRow>,
}

/// One user profile of a Jellyfin server, as a switcher chip (MEDIA-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JellyfinProfileRow {
    /// The user GUID — the profile key the switcher acts on.
    pub user_id: String,
    /// The display label (the user's name, else the id).
    pub label: String,
    /// Whether this is the active profile.
    pub active: bool,
}

/// One cached title, as an offline-list row the Sources view renders (MEDIA-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineRow {
    /// The Jellyfin item id (the play / evict key).
    pub item_id: String,
    /// The title.
    pub label: String,
    /// A human size (`"812 MB"`).
    pub size: String,
}

/// The active Jellyfin playback — the ids + method a progress report echoes back
/// (MEDIA-10 sync).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JellyfinSession {
    /// The base URL of the server the item streams from.
    pub base_url: String,
    /// The bearer token (echoed as `api_key`), if signed in.
    pub token: Option<String>,
    /// The item being played.
    pub item_id: String,
    /// The negotiated media source id.
    pub media_source_id: Option<String>,
    /// The `PlaySessionId` (when a `PlaybackInfo` opened the session).
    pub play_session_id: Option<String>,
    /// The negotiated delivery method (`PlayMethod`).
    pub method: PlaybackMethod,
    /// The default audio stream index, for the report.
    pub audio_index: Option<i32>,
    /// The default subtitle stream index, for the report.
    pub subtitle_index: Option<i32>,
}

/// The Jellyfin Sources state the controller holds (MEDIA-10).
///
/// Configuration only — the [`ServerStore`] (loaded from the `0600` token store,
/// no network) + the negotiated capability profile + the materialized items of
/// the last live browse + the active playback session. Every live call (browse,
/// `PlaybackInfo`, progress report) is a generic method that takes an injected
/// [`JellyfinClient`], so the struct carries no transport and the pure negotiation
/// / report folds are unit-tested with no network.
#[derive(Debug, Clone)]
pub struct JellyfinState {
    /// Configured servers + saved tokens.
    store: ServerStore,
    /// The player's decode profile the negotiation runs against (from the
    /// `mde-media-core` [`MpvCapabilities`] baseline by default).
    capabilities: ClientCapabilities,
    /// The materialized items of the last browse (libraries, titles, search
    /// results, or a Live-TV list). Populated by a live browse; empty until then.
    items: Vec<BaseItemDto>,
    /// The selected server id (whose base URL + token the play path uses).
    selected: Option<String>,
    /// The active playback session, for progress / stop reports.
    session: Option<JellyfinSession>,
    /// The managed offline cache (MEDIA-11) — downloaded titles + their manifest,
    /// with the add / evict / size-budget / staleness lifecycle.
    cache: OfflineCache,
}

impl Default for JellyfinState {
    fn default() -> Self {
        Self {
            store: ServerStore::new(),
            capabilities: client_capabilities(&MpvCapabilities::baseline()),
            items: Vec::new(),
            selected: None,
            session: None,
            cache: OfflineCache::new(),
        }
    }
}

impl JellyfinState {
    /// The configured servers (read-only).
    #[must_use]
    pub const fn store(&self) -> &ServerStore {
        &self.store
    }

    /// The negotiated capability profile (read-only).
    #[must_use]
    pub const fn capabilities(&self) -> &ClientCapabilities {
        &self.capabilities
    }

    /// The materialized items of the last browse.
    #[must_use]
    pub fn items(&self) -> &[BaseItemDto] {
        &self.items
    }

    /// The selected server config, if one is selected.
    #[must_use]
    pub fn selected_server(&self) -> Option<&ServerConfig> {
        self.selected.as_deref().and_then(|id| self.store.get(id))
    }

    /// The config of the server with `id`, if configured.
    #[must_use]
    pub fn server(&self, id: &str) -> Option<&ServerConfig> {
        self.store.get(id)
    }

    /// The active playback session, if one is open.
    #[must_use]
    pub const fn session(&self) -> Option<&JellyfinSession> {
        self.session.as_ref()
    }

    /// The offline cache (read-only) — the downloaded titles + budget (MEDIA-11).
    #[must_use]
    pub const fn cache(&self) -> &OfflineCache {
        &self.cache
    }
}

/// Build the Jellyfin client capability profile from the player's mpv decode set.
///
/// The §6 bridge that ties `mde-media-core`'s [`MpvCapabilities`] to the
/// negotiation input, so a title is direct-played exactly when the local player
/// can actually decode it.
#[must_use]
pub fn client_capabilities(caps: &MpvCapabilities) -> ClientCapabilities {
    ClientCapabilities::new()
        .with_containers(caps.containers().iter().map(String::as_str))
        .with_video_codecs(caps.video_codecs().iter().map(String::as_str))
        .with_audio_codecs(caps.audio_codecs().iter().map(String::as_str))
}

/// The stream media type an item plays as — music (`Audio`, `MusicAlbum`) rides
/// the `Audio` stream endpoints, everything else the `Video` ones.
#[must_use]
pub fn stream_media_type(item: &BaseItemDto) -> StreamMediaType {
    match item.item_type.as_deref() {
        Some("Audio" | "MusicAlbum" | "MusicArtist" | "MusicVideo") => StreamMediaType::Audio,
        _ => StreamMediaType::Video,
    }
}

/// A UI intent the views raise.
///
/// [`MediaController::dispatch`] maps each to the matching [`mde_media_core`] call.
/// This enum *is* the glue seam (§6) — every arm is one core method, so the surface
/// reimplements no playback / queue / index logic.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportAction {
    /// Toggle play/pause ([`Player::toggle_pause`]).
    TogglePlay,
    /// Stop + unload ([`Player::stop`]).
    Stop,
    /// Seek relative by `secs` ([`Player::seek`] around the live position).
    SeekBy(f64),
    /// Seek to an absolute `secs` ([`Player::seek`]).
    SeekTo(f64),
    /// Advance the queue ([`Player::play_next`]).
    Next,
    /// Step the queue back ([`Player::play_prev`]).
    Prev,
    /// Step one frame forward ([`Player::frame_step`]).
    FrameForward,
    /// Step one frame back ([`Player::frame_back_step`]).
    FrameBack,
    /// Snapshot the current frame ([`Player::snapshot`]).
    Snapshot(ScreenshotMode),
    /// Next chapter ([`Player::chapter_next`]).
    ChapterNext,
    /// Previous chapter ([`Player::chapter_prev`]).
    ChapterPrev,
    /// Set the playback speed ([`Player::set_controls`] with a new `speed`).
    SetSpeed(f64),
    /// Mark the next A-B loop endpoint at the live position ([`Player::set_controls`]).
    MarkAbLoop,
    /// Clear the A-B loop ([`Player::set_controls`] with [`AbLoop::Off`]).
    ClearAbLoop,
    /// Cycle the queue [`RepeatMode`] (off → all → one → off).
    ToggleRepeat,
    /// Toggle deterministic shuffle on the queue ([`Playlist::shuffle`] /
    /// [`Playlist::unshuffle`]).
    ToggleShuffle,
    /// Select one enumerated track by kind + id ([`Player::set_track_selection`]).
    SelectTrack(TrackKind, i64),
    /// Load + play a path immediately ([`Player::load`]).
    PlayPath(String),
    /// Enqueue a path with an optional title ([`Playlist::push`]).
    Enqueue(String, Option<String>),
    /// Make the queue item at this index current + load it ([`Playlist::select`] +
    /// [`Player::load`]).
    SelectQueueIndex(usize),
    /// Remove the queue item at this index ([`Playlist::remove`]).
    RemoveQueueIndex(usize),
    /// Reorder a queue item from → to ([`Playlist::reorder`]).
    MoveQueueItem(usize, usize),
    /// Clear the whole queue ([`Playlist::clear`]).
    ClearQueue,
}

/// The media surface controller.
///
/// The core's [`Player`] + [`Library`] plus the [`UiState`] the views need. Generic
/// over the engine `E` so it drives the airgap-safe `FakeMpv` in tests + the default
/// build and the real mpv engine under `--features mpv`.
#[derive(Debug)]
pub struct MediaController<E: MediaEngine> {
    /// The core player state machine (transport + MEDIA-3/4/5/6 config + the queue +
    /// resume). All playback state lives here — the surface only drives + renders it.
    player: Player<E>,
    /// The core local-media index (MEDIA-7). The surface browses it; it does not
    /// re-implement indexing.
    library: Library,
    /// The Jellyfin Sources state (MEDIA-10) — configured servers, the negotiated
    /// capability profile, the last browse, and the active playback session.
    jellyfin: JellyfinState,
    /// The non-core UI state.
    ui: UiState,
}

impl<E: MediaEngine> MediaController<E> {
    /// Wrap a core [`Player`] in a fresh controller (empty library, Sources tab).
    #[must_use]
    pub fn new(player: Player<E>) -> Self {
        Self {
            player,
            library: Library::new(),
            jellyfin: JellyfinState::default(),
            ui: UiState::default(),
        }
    }

    // ── accessors (the views + tests read the core through these) ───────────────

    /// The core player (read-only).
    #[must_use]
    pub const fn player(&self) -> &Player<E> {
        &self.player
    }

    /// The core player (mutable — the app's per-frame `pump` drives it).
    pub const fn player_mut(&mut self) -> &mut Player<E> {
        &mut self.player
    }

    /// The core library (read-only).
    #[must_use]
    pub const fn library(&self) -> &Library {
        &self.library
    }

    /// The core library (mutable — indexing a folder mutates it).
    pub const fn library_mut(&mut self) -> &mut Library {
        &mut self.library
    }

    /// The core playback queue (MEDIA-6) the Queue view renders.
    #[must_use]
    pub const fn queue(&self) -> &Playlist {
        self.player.playlist()
    }

    /// The non-core UI state (read-only).
    #[must_use]
    pub const fn ui(&self) -> &UiState {
        &self.ui
    }

    /// The non-core UI state (mutable — the views bind widgets to it).
    pub const fn ui_mut(&mut self) -> &mut UiState {
        &mut self.ui
    }

    // ── folds the views render ──────────────────────────────────────────────────

    /// The library items matching the live [`UiState::query`] — the
    /// [`Library::browse`] fold (MEDIA-7 search + kind filter + sort) the Library view
    /// lists. A pure read of the core.
    #[must_use]
    pub fn visible_items(&self) -> Vec<&LibraryItem> {
        self.library.browse(&self.ui.query)
    }

    /// The Sources rows — one per indexed root, with its item count.
    #[must_use]
    pub fn sources(&self) -> Vec<SourceRow> {
        source_rows(&self.library)
    }

    /// The enumerated tracks of the loaded media (MEDIA-5) — what the track menus list.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        self.player.tracks()
    }

    /// Whether the engine is actively playing (not paused / idle / ended).
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.player.state() == PlayerState::Playing
    }

    // ── search wiring ───────────────────────────────────────────────────────────

    /// Point the search buffer + the browse [`BrowseQuery::search`] at `needle` (an
    /// empty needle clears the filter). Keeps the view's text field and the fold in
    /// lock-step.
    pub fn set_search(&mut self, needle: impl Into<String>) {
        let needle = needle.into();
        self.ui.query.search = if needle.trim().is_empty() {
            None
        } else {
            Some(needle.clone())
        };
        self.ui.search_input = needle;
    }

    /// Restrict the Library browse to one [`MediaKind`] (or `None` for both).
    pub const fn set_kind_filter(&mut self, kind: Option<MediaKind>) {
        self.ui.query.kind = kind;
    }

    /// Set the browse [`SortKey`].
    pub const fn set_sort(&mut self, sort: SortKey) {
        self.ui.query.sort = sort;
    }

    /// Set the browse sort direction (`true` = descending).
    pub const fn set_descending(&mut self, descending: bool) {
        self.ui.query.descending = descending;
    }

    // ── indexing (the Sources "add a folder" action) ────────────────────────────

    /// Index the folder currently in [`UiState::folder_input`] into the [`Library`]
    /// (MEDIA-7 [`Library::index_folder`]) and report the outcome on the status line.
    /// A no-op with an empty field. Glue — the walk + merge live in the core.
    pub fn index_current_folder(&mut self) {
        let path = self.ui.folder_input.trim().to_owned();
        if path.is_empty() {
            self.ui.status = Some("Enter a folder path to index.".to_owned());
            return;
        }
        match self.library.index_folder(&path) {
            Ok(added) => {
                self.ui.status = Some(format!("Indexed {path}: {added} new item(s)."));
                self.ui.folder_input.clear();
            }
            Err(e) => self.ui.status = Some(format!("Could not index {path}: {e}")),
        }
    }

    // ── network streams + yt-dlp (MEDIA-12) ──────────────────────────────────────

    /// Open the URL / path in [`UiState::url_input`] (MEDIA-12): classify it with the
    /// core [`classify_url`] fold, then route it. Direct streams + local files are
    /// handed straight to the core [`Player::load`] (mpv plays `http(s)`/`hls`/`rtsp`/
    /// `mms`/`rtmp`/`srt` natively); a web page is resolved through the injected
    /// [`YtDlpResolver`] seam and its direct URL loaded. Reports every outcome
    /// honestly on the status line — an unsupported string, and (§7) an absent
    /// `yt-dlp` — never a stub. Glue: the resolve → play path reuses the same
    /// `Player::load` the local + Jellyfin paths do.
    ///
    /// # Errors
    /// A status string when the input is not playable, `yt-dlp` is absent / fails,
    /// or the core rejects the load. On success the field is cleared and the surface
    /// jumps to the Player view.
    pub fn open_url<R: YtDlpResolver>(&mut self, input: &str, resolver: &R) -> Result<(), String> {
        let target = input.trim().to_owned();
        match classify_url(&target) {
            UrlKind::DirectStream | UrlKind::LocalFile => {
                self.player.load(target.clone()).map_err(err)?;
                self.finish_open(format!("Opening {target}"));
                Ok(())
            }
            UrlKind::WebPage => self.open_web_page(&target, resolver),
            UrlKind::Invalid => {
                let msg = format!("Not a stream URL or web link: {target}");
                self.ui.status = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// Resolve a web-page URL through `yt-dlp` and play its direct media URL — the
    /// [`UrlKind::WebPage`] arm of [`open_url`](Self::open_url). Honest-gates on the
    /// tool being present (§7) before invoking it.
    fn open_web_page<R: YtDlpResolver>(&mut self, page: &str, resolver: &R) -> Result<(), String> {
        if !resolver.is_available() {
            let msg = "yt-dlp not installed — install it to open web videos (streams still work)."
                .to_owned();
            self.ui.status = Some(msg.clone());
            return Err(msg);
        }
        let media = resolver.resolve(page).map_err(ytdlp_err)?;
        let url = media
            .primary()
            .ok_or_else(|| "yt-dlp resolved no playable media URL.".to_owned())?
            .to_owned();
        self.player.load(url).map_err(err)?;
        let title = media.title.unwrap_or_else(|| page.to_owned());
        self.finish_open(format!("Playing {title}"));
        Ok(())
    }

    /// Shared success tail of an open: report it, clear the field, jump to Player.
    fn finish_open(&mut self, status: String) {
        self.ui.status = Some(status);
        self.ui.url_input.clear();
        self.ui.tab = MediaTab::Player;
    }

    // ── the per-frame pump ──────────────────────────────────────────────────────

    /// Advance the core one tick ([`Player::pump`]) and fold any surfaced
    /// [`PlayerEvent::Error`] onto the status line. Called at the top of every frame.
    pub fn pump(&mut self) {
        self.player.pump();
        for event in self.player.drain_events() {
            if let PlayerEvent::Error(msg) = event {
                self.ui.status = Some(msg);
            }
        }
    }

    // ── the transport glue ──────────────────────────────────────────────────────

    /// Apply a [`TransportAction`] to the core, recording any refusal on the status
    /// line. This is the whole glue seam (§6): each arm is one core call — the surface
    /// reimplements no playback, queue, or index logic.
    pub fn dispatch(&mut self, action: TransportAction) {
        if let Err(msg) = self.apply(action) {
            self.ui.status = Some(msg);
        }
    }

    /// The fallible body of [`dispatch`](Self::dispatch): map the intent to a core call.
    fn apply(&mut self, action: TransportAction) -> Result<(), String> {
        match action {
            TransportAction::TogglePlay => self.player.toggle_pause().map_err(err)?,
            TransportAction::Stop => self.player.stop().map_err(err)?,
            TransportAction::SeekBy(delta) => {
                let target = (self.player.position() + delta).max(0.0);
                self.player.seek(target).map_err(err)?;
            }
            TransportAction::SeekTo(secs) => self.player.seek(secs).map_err(err)?,
            TransportAction::Next => {
                self.player.play_next().map_err(err)?;
            }
            TransportAction::Prev => {
                self.player.play_prev().map_err(err)?;
            }
            TransportAction::FrameForward => self.player.frame_step().map_err(err)?,
            TransportAction::FrameBack => self.player.frame_back_step().map_err(err)?,
            TransportAction::Snapshot(mode) => {
                self.player.snapshot(mode).map_err(err)?;
                self.ui.status = Some("Snapshot captured.".to_owned());
            }
            TransportAction::ChapterNext => self.player.chapter_next().map_err(err)?,
            TransportAction::ChapterPrev => self.player.chapter_prev().map_err(err)?,
            TransportAction::SetSpeed(speed) => {
                let mut controls: PlaybackControls = *self.player.controls();
                controls.speed = speed;
                self.player.set_controls(controls).map_err(err)?;
            }
            TransportAction::MarkAbLoop => self.mark_ab_loop()?,
            TransportAction::ClearAbLoop => {
                let mut controls: PlaybackControls = *self.player.controls();
                controls.ab_loop = AbLoop::Off;
                self.ui.ab_pending = None;
                self.player.set_controls(controls).map_err(err)?;
                self.ui.status = Some("A-B loop cleared.".to_owned());
            }
            TransportAction::ToggleRepeat => {
                let next = next_repeat(self.player.playlist().repeat());
                self.player.playlist_mut().set_repeat(next);
            }
            TransportAction::ToggleShuffle => {
                if self.player.playlist().is_shuffled() {
                    self.player.playlist_mut().unshuffle();
                } else {
                    self.player.playlist_mut().shuffle(SHUFFLE_SEED);
                }
            }
            TransportAction::SelectTrack(kind, id) => {
                let mut selection = self.player.track_selection().clone();
                set_track(&mut selection, kind, TrackSelect::Id(id));
                self.player.set_track_selection(selection).map_err(err)?;
            }
            TransportAction::PlayPath(url) => self.player.load(url).map_err(err)?,
            TransportAction::Enqueue(url, title) => {
                let item = title.map_or_else(
                    || PlaylistItem::new(url.clone()),
                    |t| PlaylistItem::titled(url.clone(), t),
                );
                self.player.playlist_mut().push(item);
            }
            TransportAction::SelectQueueIndex(index) => {
                if self.player.playlist_mut().select(index) {
                    if let Some(url) = self.player.playlist().current().map(|i| i.url.clone()) {
                        self.player.load(url).map_err(err)?;
                    }
                }
            }
            TransportAction::RemoveQueueIndex(index) => {
                self.player.playlist_mut().remove(index);
            }
            TransportAction::MoveQueueItem(from, to) => {
                self.player.playlist_mut().reorder(from, to);
            }
            TransportAction::ClearQueue => self.player.playlist_mut().clear(),
        }
        Ok(())
    }

    /// The A-B loop state machine: the first mark records A at the live position; the
    /// second sets the [`AbLoop::Range`] (ordered) through [`Player::set_controls`].
    fn mark_ab_loop(&mut self) -> Result<(), String> {
        let pos = self.player.position();
        match self.ui.ab_pending.take() {
            None => {
                self.ui.ab_pending = Some(pos);
                self.ui.status = Some("A-B loop: A set — mark B next.".to_owned());
            }
            Some(a) => {
                let (lo, hi) = if a <= pos { (a, pos) } else { (pos, a) };
                let mut controls: PlaybackControls = *self.player.controls();
                controls.ab_loop = AbLoop::Range { a: lo, b: hi };
                self.player.set_controls(controls).map_err(err)?;
                self.ui.status = Some("A-B loop on.".to_owned());
            }
        }
        Ok(())
    }

    // ── Jellyfin Sources (MEDIA-10) ──────────────────────────────────────────────

    /// The Jellyfin Sources state (read-only) — configured servers, the negotiated
    /// capability profile, the last browse, and the active session.
    #[must_use]
    pub const fn jellyfin(&self) -> &JellyfinState {
        &self.jellyfin
    }

    /// Replace the configured Jellyfin servers — e.g. after a
    /// [`ServerStore::load`] at startup (no network).
    pub fn set_jellyfin_store(&mut self, store: ServerStore) {
        self.jellyfin.store = store;
    }

    /// Override the negotiated capability profile (the default is the
    /// `mde-media-core` mpv baseline) — e.g. to reflect a constrained seat.
    pub fn set_jellyfin_capabilities(&mut self, caps: ClientCapabilities) {
        self.jellyfin.capabilities = caps;
    }

    /// Add / update a configured Jellyfin server (no network) — the Sources
    /// "add a server" affordance.
    pub fn add_jellyfin_server(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
    ) {
        self.jellyfin
            .store
            .upsert(ServerConfig::new(id, name, base_url));
    }

    /// Select the server future play actions stream from. A no-op for an unknown id.
    pub fn select_jellyfin_server(&mut self, id: &str) {
        if self.jellyfin.store.get(id).is_some() {
            self.jellyfin.selected = Some(id.to_owned());
        }
    }

    // ── Jellyfin user profiles (MEDIA-11) ─────────────────────────────────────

    /// Add / refresh a user profile on a server (each its own token + user), the
    /// store side of a per-server sign-in. Returns whether the server exists.
    pub fn add_jellyfin_profile(&mut self, server_id: &str, auth: ServerAuth) -> bool {
        self.jellyfin.store.add_profile(server_id, auth)
    }

    /// Switch the active user profile on a server, so subsequent browse / play use
    /// that profile's token. Reports the outcome on the status line; returns
    /// whether the switch happened.
    pub fn switch_jellyfin_profile(&mut self, server_id: &str, user_id: &str) -> bool {
        let switched = self.jellyfin.store.switch_profile(server_id, user_id);
        if switched {
            let who = self
                .jellyfin
                .store
                .get(server_id)
                .and_then(ServerConfig::active_auth)
                .map_or_else(|| user_id.to_owned(), profile_label);
            self.ui.status = Some(format!("Switched to {who}."));
        } else {
            self.ui.status = Some("No such profile on that server.".to_owned());
        }
        switched
    }

    // ── Jellyfin offline cache (MEDIA-11) ─────────────────────────────────────

    /// Point the offline cache at `root` (the tests use a scratch dir; the app uses
    /// the default under the config dir) and reload its manifest. Reports a load
    /// failure honestly.
    pub fn set_jellyfin_offline_root(&mut self, root: impl Into<std::path::PathBuf>) {
        let root = root.into();
        match OfflineCache::load_from(&root) {
            Ok(cache) => self.jellyfin.cache = cache,
            Err(e) => {
                self.jellyfin.cache = OfflineCache::with_root(root);
                self.ui.status = Some(format!("Offline cache: {e}"));
            }
        }
    }

    /// Whether `item_id` is downloaded for offline playback.
    #[must_use]
    pub fn is_offline_available(&self, item_id: &str) -> bool {
        self.jellyfin.cache.contains(item_id)
    }

    /// The offline-list rows the Sources view renders — one per downloaded title.
    #[must_use]
    pub fn offline_rows(&self) -> Vec<OfflineRow> {
        self.jellyfin
            .cache
            .entries()
            .iter()
            .map(|entry| OfflineRow {
                item_id: entry.item_id.clone(),
                label: entry.title.clone(),
                size: human_bytes(entry.byte_len),
            })
            .collect()
    }

    /// The offline cache usage as a `"used / budget"` label for the Sources view.
    #[must_use]
    pub fn offline_usage(&self) -> String {
        let used = human_bytes(self.jellyfin.cache.total_bytes());
        self.jellyfin.cache.size_budget().map_or_else(
            || format!("{used} offline"),
            |budget| format!("{used} / {} offline", human_bytes(budget)),
        )
    }

    /// Download `item`'s untouched direct-play bytes through the client and store
    /// them in the offline cache (MEDIA-11) — the download→cache half of the offline
    /// path. Reuses the client transport seam ([`JellyfinClient::download`]) + the
    /// managed [`OfflineCache`]; a live server is honest-gated, tests drive it
    /// through a fixture transport with synthetic bytes.
    ///
    /// # Errors
    /// A status string when no server is selected, the item has no source, the
    /// download fails, or the cache write fails.
    pub fn download_jellyfin_item<T: HttpTransport>(
        &mut self,
        client: &JellyfinClient<T>,
        item: &BaseItemDto,
        now: u64,
    ) -> Result<CacheEntry, String> {
        let (base_url, token) = self.selected_endpoint()?;
        let server_id = self
            .jellyfin
            .selected
            .clone()
            .ok_or_else(|| "Select a Jellyfin server first.".to_owned())?;
        let Some(source) = item.media_sources.first() else {
            return Err(format!(
                "{} has no downloadable source yet — browse the library first.",
                jellyfin_item_title(item)
            ));
        };
        // The untouched original bytes (static direct-play) so the file plays
        // offline with no server transcode.
        let url = direct_play_url(
            &base_url,
            &item.id,
            source.id.as_deref(),
            stream_media_type(item),
            token.as_deref(),
        );
        let bytes = client.download(&url).map_err(jellyfin_err)?;
        let request = CacheRequest {
            item_id: item.id.clone(),
            server_id,
            source_id: source.id.clone(),
            title: jellyfin_item_title(item),
            container: source.container.clone().unwrap_or_else(|| "bin".to_owned()),
        };
        let entry = self
            .jellyfin
            .cache
            .store(&request, &bytes, now)
            .map_err(|e| format!("Offline cache: {e}"))?;
        self.ui.status = Some(format!(
            "Downloaded {} for offline ({}).",
            entry.title,
            human_bytes(entry.byte_len)
        ));
        Ok(entry)
    }

    /// Play a downloaded title from the offline cache (MEDIA-11) — load its local
    /// file into the core [`Player`] and bump its LRU last-access. No network: the
    /// offline half of the path.
    ///
    /// # Errors
    /// A status string when the item is not cached or the core rejects the load.
    pub fn play_offline_item(&mut self, item_id: &str, now: u64) -> Result<(), String> {
        let path = self
            .jellyfin
            .cache
            .local_path(item_id)
            .ok_or_else(|| format!("{item_id} is not downloaded for offline playback."))?;
        let url = path.to_string_lossy().into_owned();
        self.player.load(url).map_err(err)?;
        // Best-effort LRU touch; a manifest write failure must not fail playback.
        let _ = self.jellyfin.cache.touch(item_id, now);
        self.ui.status = Some(format!(
            "Playing {} offline.",
            self.jellyfin
                .cache
                .get(item_id)
                .map_or(item_id, |e| e.title.as_str())
        ));
        Ok(())
    }

    /// Evict a downloaded title from the offline cache (delete its file + manifest
    /// row). Reports the outcome honestly.
    pub fn evict_offline_item(&mut self, item_id: &str) {
        match self.jellyfin.cache.evict(item_id) {
            Ok(Some(entry)) => {
                self.ui.status = Some(format!("Removed {} from offline.", entry.title));
            }
            Ok(None) => {}
            Err(e) => self.ui.status = Some(format!("Offline cache: {e}")),
        }
    }

    /// The Jellyfin server rows the Sources view renders (with their user
    /// profiles, MEDIA-11).
    #[must_use]
    pub fn jellyfin_sources(&self) -> Vec<JellyfinSourceRow> {
        let selected = self.jellyfin.selected.as_deref();
        self.jellyfin
            .store
            .servers
            .iter()
            .map(|server| {
                let active_id = server.active_profile.as_deref();
                let profiles = server
                    .profiles()
                    .iter()
                    .map(|p| JellyfinProfileRow {
                        user_id: p.user_id.clone(),
                        label: profile_label(p),
                        active: active_id == Some(p.user_id.as_str()),
                    })
                    .collect();
                JellyfinSourceRow {
                    id: server.id.clone(),
                    label: server.name.clone(),
                    base_url: server.base_url.clone(),
                    signed_in: server.is_authenticated(),
                    selected: selected == Some(server.id.as_str()),
                    active_profile: server.active_auth().map(profile_label),
                    profiles,
                }
            })
            .collect()
    }

    /// The materialized items of the last Jellyfin browse — the playable rows.
    #[must_use]
    pub fn jellyfin_items(&self) -> &[BaseItemDto] {
        self.jellyfin.items()
    }

    /// Browse a Jellyfin server through its typed client, materializing the items
    /// (a library's titles / a search / a channel list). Returns the count.
    ///
    /// A real call into `mde-jellyfin`'s client — a live server is honest-gated;
    /// tests drive it through a fixture transport.
    ///
    /// # Errors
    /// The Jellyfin error, mapped to a status string.
    pub fn browse_jellyfin<T: HttpTransport>(
        &mut self,
        client: &JellyfinClient<T>,
        query: &ItemsQuery,
    ) -> Result<usize, String> {
        let resp = client.items(query).map_err(jellyfin_err)?;
        let count = resp.items.len();
        self.jellyfin.items = resp.items;
        Ok(count)
    }

    /// Materialize a Jellyfin server's Live-TV channels (MEDIA-10). Honest-gated
    /// to a server with a tuner; the request + parse are tested in `mde-jellyfin`.
    ///
    /// # Errors
    /// The Jellyfin error, mapped to a status string.
    pub fn load_jellyfin_live_tv<T: HttpTransport>(
        &mut self,
        client: &JellyfinClient<T>,
    ) -> Result<usize, String> {
        let resp = client.live_tv_channels().map_err(jellyfin_err)?;
        let count = resp.items.len();
        self.jellyfin.items = resp.items;
        Ok(count)
    }

    /// Negotiate + play `item` from the selected server (MEDIA-10).
    ///
    /// Picks the item's first [`MediaSourceInfo`], chooses direct-play /
    /// direct-stream / transcode from the player's decode capabilities, loads the
    /// negotiated URL into the core [`Player`], and opens a sync session. Pure (no
    /// network) — negotiation + load are unit-tested.
    ///
    /// # Errors
    /// A status string when no server is selected, the item has no source, or the
    /// core rejects the load.
    pub fn play_jellyfin_item(&mut self, item: &BaseItemDto) -> Result<PlaybackDecision, String> {
        let (base_url, token) = self.selected_endpoint()?;
        let Some(source) = item.media_sources.first() else {
            return Err(format!(
                "{} has no playable source yet — browse the library first.",
                jellyfin_item_title(item)
            ));
        };
        let media_type = stream_media_type(item);
        let decision = self.negotiate_and_load(
            &base_url,
            token.as_deref(),
            &item.id,
            source,
            media_type,
            None,
        )?;
        self.ui.status = Some(format!(
            "Playing {} · {}",
            jellyfin_item_title(item),
            decision.method.as_wire()
        ));
        Ok(decision)
    }

    /// Open + play a Jellyfin item end-to-end through the client (MEDIA-10, the
    /// full live path): resolve the sources via `PlaybackInfo`, negotiate, load the
    /// [`Player`], and report the playback start.
    ///
    /// A real transport-driving call (honest-gated); tests drive it through a
    /// fixture transport.
    ///
    /// # Errors
    /// The Jellyfin / core error as a status string.
    pub fn open_jellyfin_item<T: HttpTransport>(
        &mut self,
        client: &JellyfinClient<T>,
        base_url: &str,
        token: Option<&str>,
        item_id: &str,
        media_type: StreamMediaType,
    ) -> Result<PlaybackDecision, String> {
        let info = client
            .playback_info(item_id, &self.jellyfin.capabilities)
            .map_err(jellyfin_err)?;
        let Some(source) = info.media_sources.first() else {
            return Err(format!("Server returned no playable source for {item_id}."));
        };
        let play_session = info.play_session_id.clone();
        let decision = self.negotiate_and_load(
            base_url,
            token,
            item_id,
            source,
            media_type,
            play_session.as_deref(),
        )?;
        if let Some(report) = self.jellyfin_progress_report() {
            client
                .report_playback_start(&report)
                .map_err(jellyfin_err)?;
        }
        Ok(decision)
    }

    /// Build a progress report for the active Jellyfin session at the live
    /// position, or [`None`] when no session is open. Pure (testable).
    #[must_use]
    pub fn jellyfin_progress_report(&self) -> Option<PlaybackReport> {
        self.jellyfin
            .session
            .as_ref()
            .map(|session| self.session_report(session))
    }

    /// Report the active session's progress through the client (MEDIA-10 sync —
    /// advances the server-side resume point). Honest-gated to a live server.
    ///
    /// # Errors
    /// The Jellyfin error as a status string.
    pub fn report_jellyfin_progress<T: HttpTransport>(
        &self,
        client: &JellyfinClient<T>,
    ) -> Result<(), String> {
        self.jellyfin_progress_report().map_or(Ok(()), |report| {
            client
                .report_playback_progress(&report)
                .map_err(jellyfin_err)
        })
    }

    /// End the active Jellyfin session, reporting the final position through the
    /// client and clearing the session. A no-op when no session is open.
    ///
    /// # Errors
    /// The Jellyfin error as a status string.
    pub fn report_jellyfin_stopped<T: HttpTransport>(
        &mut self,
        client: &JellyfinClient<T>,
    ) -> Result<(), String> {
        let Some(report) = self.end_jellyfin_session() else {
            return Ok(());
        };
        client
            .report_playback_stopped(&report)
            .map_err(jellyfin_err)
    }

    /// Take the active session (clearing it), returning its final stop report.
    /// Pure (testable) — the report side of "stop this title".
    pub fn end_jellyfin_session(&mut self) -> Option<PlaybackReport> {
        let report = self
            .jellyfin
            .session
            .as_ref()
            .map(|session| self.session_report(session));
        self.jellyfin.session = None;
        report
    }

    /// The base URL + token of the selected server, or a status error.
    fn selected_endpoint(&self) -> Result<(String, Option<String>), String> {
        let server = self
            .jellyfin
            .selected_server()
            .ok_or_else(|| "Select a Jellyfin server first.".to_owned())?;
        let token = server.auth.as_ref().map(|auth| auth.access_token.clone());
        Ok((server.base_url.clone(), token))
    }

    /// Negotiate `source` against the capability profile, load the resulting URL,
    /// and record the sync session — the shared body of the two play paths.
    fn negotiate_and_load(
        &mut self,
        base_url: &str,
        token: Option<&str>,
        item_id: &str,
        source: &MediaSourceInfo,
        media_type: StreamMediaType,
        play_session_id: Option<&str>,
    ) -> Result<PlaybackDecision, String> {
        let decision = build_playback_decision(
            base_url,
            item_id,
            source,
            &self.jellyfin.capabilities,
            media_type,
            token,
            play_session_id,
        );
        self.player.load(decision.url.clone()).map_err(err)?;
        self.jellyfin.session = Some(JellyfinSession {
            base_url: base_url.to_owned(),
            token: token.map(ToOwned::to_owned),
            item_id: item_id.to_owned(),
            media_source_id: decision.media_source_id.clone(),
            play_session_id: decision.play_session_id.clone(),
            method: decision.method,
            audio_index: source.default_audio_index(),
            subtitle_index: source.default_subtitle_index(),
        });
        Ok(decision)
    }

    /// Build a [`PlaybackReport`] for `session` at the live player position + state.
    fn session_report(&self, session: &JellyfinSession) -> PlaybackReport {
        let mut report = PlaybackReport::new(&session.item_id)
            .with_session(
                session.media_source_id.clone(),
                session.play_session_id.clone(),
            )
            .with_method(session.method)
            .paused(self.player.state() != PlayerState::Playing)
            .at_secs(self.player.position());
        report.audio_stream_index = session.audio_index;
        report.subtitle_stream_index = session.subtitle_index;
        report
    }
}

/// Map a [`mde_media_core::PlayerError`] to a status string. Taken by value so the
/// call sites stay the point-free `.map_err(err)` form.
#[allow(clippy::needless_pass_by_value)]
fn err(e: mde_media_core::PlayerError) -> String {
    e.to_string()
}

/// Map a [`JellyfinError`] to a status string (point-free `.map_err(jellyfin_err)`).
#[allow(clippy::needless_pass_by_value)]
fn jellyfin_err(e: JellyfinError) -> String {
    format!("Jellyfin: {e}")
}

/// Map a [`YtDlpError`] to an honest status string (MEDIA-12). The tool-absent case
/// gets a plain install hint rather than a raw error (§7).
#[allow(clippy::needless_pass_by_value)]
fn ytdlp_err(e: YtDlpError) -> String {
    match e {
        YtDlpError::NotInstalled => {
            "yt-dlp not installed — install it to open web videos.".to_owned()
        }
        other => format!("yt-dlp: {other}"),
    }
}

/// The display title of a Jellyfin item — its name, else its id (never empty).
#[must_use]
pub fn jellyfin_item_title(item: &BaseItemDto) -> String {
    item.name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| item.id.clone())
}

/// A user profile's display label — its user name, else its user id (MEDIA-11).
#[must_use]
pub fn profile_label(auth: &ServerAuth) -> String {
    auth.user_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| auth.user_id.clone())
}

/// A compact human byte size (`"0 B"`, `"812 MB"`, `"1.4 GB"`) for the offline
/// list. Binary units (1024) matching the cache budget; a pure fold, so it is
/// unit-tested.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    // One decimal below 10 (1.4 GB), none above (812 MB) — a tidy, stable width.
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Put `select` into the `kind` slot of a [`TrackSelection`].
const fn set_track(selection: &mut TrackSelection, kind: TrackKind, select: TrackSelect) {
    match kind {
        TrackKind::Audio => selection.audio = select,
        TrackKind::Video => selection.video = select,
        TrackKind::Subtitle => selection.subtitle = select,
    }
}

/// The next repeat mode in the UI cycle: off → all → one → off.
#[must_use]
pub const fn next_repeat(mode: RepeatMode) -> RepeatMode {
    match mode {
        RepeatMode::Off => RepeatMode::All,
        RepeatMode::All => RepeatMode::One,
        RepeatMode::One => RepeatMode::Off,
    }
}

/// The Sources rows for a [`Library`]: one per indexed root, each with the count of
/// items whose path sits under it. A pure fold, so it is unit-tested.
#[must_use]
pub fn source_rows(library: &Library) -> Vec<SourceRow> {
    library
        .roots()
        .iter()
        .map(|root| {
            let item_count = library
                .items()
                .filter(|item| item.path.starts_with(root.as_str()))
                .count();
            SourceRow {
                label: source_label(root),
                path: root.clone(),
                item_count,
            }
        })
        .collect()
}

/// A short display name for a source root — its final path component.
#[must_use]
pub fn source_label(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// The `(title, subtitle)` a Library row renders: the title, then a `kind · duration ·
/// artist · album` line omitting any part the metadata does not carry. A pure fold.
#[must_use]
pub fn library_row_texts(item: &LibraryItem) -> (String, String) {
    let meta = &item.metadata;
    let mut parts: Vec<String> = Vec::new();
    parts.push(kind_word(meta.kind).to_owned());
    if let Some(secs) = meta.duration_secs {
        parts.push(format_time(secs));
    }
    if let Some(artist) = meta.artist.as_deref() {
        parts.push(artist.to_owned());
    }
    if let Some(album) = meta.album.as_deref() {
        parts.push(album.to_owned());
    }
    (meta.title.clone(), parts.join(" · "))
}

/// The display word for a [`MediaKind`].
#[must_use]
pub const fn kind_word(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => "Audio",
        MediaKind::Video => "Video",
    }
}

/// A one-line label for a track in the track menu — id, kind, then language / title /
/// codec when present (`"#2 audio · jpn · aac"`).
#[must_use]
pub fn track_label(track: &Track) -> String {
    let mut parts: Vec<String> = vec![format!("#{} {}", track.id, track_kind_word(track.kind))];
    if let Some(lang) = track.lang.as_deref() {
        parts.push(lang.to_owned());
    }
    if let Some(title) = track.title.as_deref() {
        parts.push(title.to_owned());
    } else if let Some(codec) = track.codec.as_deref() {
        parts.push(codec.to_owned());
    }
    parts.join(" · ")
}

/// The lowercase word for a [`TrackKind`] used in a track label.
#[must_use]
pub const fn track_kind_word(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Subtitle => "sub",
    }
}

/// Format a duration in seconds as `M:SS` (or `H:MM:SS` past an hour). Non-finite /
/// negative inputs render `0:00`. Pure + deterministic, so it is unit-tested.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn format_time(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 {
        secs as u64
    } else {
        0
    };
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The scrubber fill fraction `[0, 1]` for a position + known duration (`0` when the
/// duration is unknown or non-positive). Pure, so the transport bar is testable.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn progress_fraction(position: f64, duration: Option<f64>) -> f32 {
    match duration {
        Some(dur) if dur > 0.0 => (position / dur).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    }
}

/// Whether the auto-hiding media OSD should be visible.
///
/// Always while paused (so the operator can act), else only within [`OSD_HIDE_SECS`]
/// of the last pointer activity. Pure, so the auto-hide policy is unit-tested without
/// a real clock (design Q32).
#[must_use]
pub const fn osd_should_show(idle_secs: f64, paused: bool) -> bool {
    paused || idle_secs < OSD_HIDE_SECS
}

/// The transport verb label for the play/pause button given the live state.
#[must_use]
pub const fn play_pause_label(state: PlayerState) -> &'static str {
    match state {
        PlayerState::Playing => "Pause",
        _ => "Play",
    }
}

/// A short human word for the current [`PlayerState`], for the status chrome.
#[must_use]
pub const fn state_word(state: PlayerState) -> &'static str {
    match state {
        PlayerState::Idle => "Idle",
        PlayerState::Loading => "Loading",
        PlayerState::Playing => "Playing",
        PlayerState::Paused => "Paused",
        PlayerState::Stopped => "Stopped",
        PlayerState::Ended => "Ended",
    }
}

/// The repeat-mode label for the queue button.
#[must_use]
pub const fn repeat_label(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::Off => "Repeat: off",
        RepeatMode::All => "Repeat: all",
        RepeatMode::One => "Repeat: one",
    }
}

/// The title a queue / now-playing row shows: the item's explicit title, else a
/// cleaned name derived from its URL/path (never an empty row).
#[must_use]
pub fn item_title(item: &PlaylistItem) -> String {
    item.title
        .clone()
        .unwrap_or_else(|| title_from_url(&item.url))
}

/// Derive a display title from a media URL/path — the final component without its
/// extension, underscores turned to spaces; falls back to the whole string.
#[must_use]
pub fn title_from_url(url: &str) -> String {
    let name = url
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(url);
    let stem = name.rsplit_once('.').map_or(name, |(head, _)| head);
    let cleaned = stem.replace('_', " ");
    if cleaned.trim().is_empty() {
        url.to_owned()
    } else {
        cleaned
    }
}

/// The now-playing title for the header / OSD: the loaded media's derived title, or a
/// resting label when nothing is loaded.
#[must_use]
pub fn now_playing_title<E: MediaEngine>(player: &Player<E>) -> String {
    player
        .media()
        .map_or_else(|| "Nothing playing".to_owned(), title_from_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_jellyfin::{ClientInfo, HttpRequest, HttpResponse, MediaStream, TransportError};
    use mde_media_core::{FakeMpv, MediaMetadata};

    fn tracks() -> Vec<Track> {
        vec![
            Track {
                id: 1,
                kind: TrackKind::Video,
                title: Some("Main".into()),
                lang: None,
                codec: Some("h264".into()),
                default: true,
                selected: true,
            },
            Track {
                id: 1,
                kind: TrackKind::Audio,
                title: None,
                lang: Some("eng".into()),
                codec: Some("aac".into()),
                default: true,
                selected: true,
            },
            Track {
                id: 1,
                kind: TrackKind::Subtitle,
                title: None,
                lang: Some("eng".into()),
                codec: Some("ass".into()),
                default: false,
                selected: false,
            },
        ]
    }

    /// A controller over a `FakeMpv` with a known duration + tracks — the airgap-safe
    /// engine the default build drives, so these glue tests exercise the real core.
    fn controller() -> MediaController<FakeMpv> {
        MediaController::new(Player::new(
            FakeMpv::new().with_duration(120.0).with_tracks(tracks()),
        ))
    }

    fn loaded() -> MediaController<FakeMpv> {
        let mut c = controller();
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump(); // FileLoaded → Playing
        c
    }

    // ── network streams + yt-dlp (MEDIA-12) ──────────────────────────────────────

    /// A fake `yt-dlp` seam: scripted availability + a fixed [`ResolvedMedia`], so
    /// the open-URL glue is driven with no real tool and no network.
    struct FakeResolver {
        available: bool,
        resolved: mde_media_core::ResolvedMedia,
    }

    impl FakeResolver {
        /// An available resolver that returns `url` (titled `title`) for any page.
        fn ready(title: &str, url: &str) -> Self {
            Self {
                available: true,
                resolved: mde_media_core::ResolvedMedia {
                    source_url: String::new(),
                    title: Some(title.to_owned()),
                    urls: vec![url.to_owned()],
                },
            }
        }

        /// An absent resolver (the honest tool-absent gate).
        const fn absent() -> Self {
            Self {
                available: false,
                resolved: mde_media_core::ResolvedMedia {
                    source_url: String::new(),
                    title: None,
                    urls: Vec::new(),
                },
            }
        }
    }

    impl YtDlpResolver for FakeResolver {
        fn is_available(&self) -> bool {
            self.available
        }

        fn resolve(&self, page_url: &str) -> Result<mde_media_core::ResolvedMedia, YtDlpError> {
            if !self.available {
                return Err(YtDlpError::NotInstalled);
            }
            let mut out = self.resolved.clone();
            out.source_url = page_url.to_owned();
            Ok(out)
        }
    }

    #[test]
    fn open_url_direct_stream_loads_the_core_player() {
        let mut c = controller();
        // A direct stream URL is handed straight to the core Player (no yt-dlp).
        c.open_url("rtsp://cam.mesh:554/live", &FakeResolver::absent())
            .expect("direct stream opens without a resolver");
        assert_eq!(c.player().media(), Some("rtsp://cam.mesh:554/live"));
        assert_eq!(c.player().state(), PlayerState::Loading);
        assert_eq!(c.ui().tab, MediaTab::Player);
        assert!(c.ui().url_input.is_empty(), "the field is cleared on open");
        c.pump();
        assert_eq!(c.player().state(), PlayerState::Playing);
    }

    #[test]
    fn open_url_http_media_file_is_a_direct_stream() {
        let mut c = controller();
        c.open_url("https://cdn.example/movie.mp4", &FakeResolver::absent())
            .expect("an http media file plays directly");
        assert_eq!(c.player().media(), Some("https://cdn.example/movie.mp4"));
    }

    #[test]
    fn open_url_local_path_loads_the_core_player() {
        let mut c = controller();
        c.open_url("/media/movies/clip.mkv", &FakeResolver::absent())
            .expect("a local path opens");
        assert_eq!(c.player().media(), Some("/media/movies/clip.mkv"));
    }

    #[test]
    fn open_url_web_page_resolves_via_ytdlp_then_plays() {
        let mut c = controller();
        let resolver = FakeResolver::ready(
            "Never Gonna Give You Up",
            "https://cdn.example/direct-stream.mp4",
        );
        c.open_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &resolver)
            .expect("a web page resolves + plays");
        // The core Player loaded the *resolved* direct URL, not the web page.
        assert_eq!(
            c.player().media(),
            Some("https://cdn.example/direct-stream.mp4")
        );
        assert_eq!(c.player().state(), PlayerState::Loading);
        assert_eq!(c.ui().tab, MediaTab::Player);
        assert_eq!(
            c.ui().status.as_deref(),
            Some("Playing Never Gonna Give You Up")
        );
    }

    #[test]
    fn open_url_web_page_is_honest_gated_when_ytdlp_is_absent() {
        let mut c = controller();
        let err = c
            .open_url("https://youtu.be/dQw4w9WgXcQ", &FakeResolver::absent())
            .expect_err("no yt-dlp → honest refusal, not a stub");
        assert!(
            err.contains("yt-dlp not installed"),
            "honest message: {err}"
        );
        // Nothing loaded, the field is kept so the operator can retry.
        assert_eq!(c.player().state(), PlayerState::Idle);
        assert_eq!(c.player().media(), None);
        assert!(c
            .ui()
            .status
            .as_deref()
            .unwrap_or_default()
            .contains("yt-dlp"));
    }

    #[test]
    fn open_url_invalid_input_surfaces_honestly_and_loads_nothing() {
        let mut c = controller();
        let err = c
            .open_url("mailto:someone@example.com", &FakeResolver::absent())
            .expect_err("an unsupported scheme is refused");
        assert!(err.contains("Not a stream URL"), "honest message: {err}");
        assert_eq!(c.player().state(), PlayerState::Idle);
        assert_eq!(c.player().media(), None);
    }

    #[test]
    fn open_url_resolver_failure_surfaces_on_the_status_line() {
        // An available resolver that returns no URL → an honest NoMedia refusal.
        struct EmptyResolver;
        impl YtDlpResolver for EmptyResolver {
            fn is_available(&self) -> bool {
                true
            }
            fn resolve(&self, _: &str) -> Result<mde_media_core::ResolvedMedia, YtDlpError> {
                Err(YtDlpError::Failed("Unsupported URL".to_owned()))
            }
        }
        let mut c = controller();
        let err = c
            .open_url("https://example.com/article", &EmptyResolver)
            .expect_err("a yt-dlp failure surfaces");
        assert!(err.starts_with("yt-dlp:"), "honest message: {err}");
        assert_eq!(c.player().state(), PlayerState::Idle);
    }

    // ── transport glue (the surface drives the core) ─────────────────────────────

    #[test]
    fn play_path_loads_into_the_core_player() {
        let mut c = controller();
        assert_eq!(c.player().state(), PlayerState::Idle);
        c.dispatch(TransportAction::PlayPath("movie.mkv".to_owned()));
        assert_eq!(c.player().media(), Some("movie.mkv"));
        c.pump();
        assert_eq!(c.player().state(), PlayerState::Playing);
    }

    #[test]
    fn toggle_play_pauses_and_resumes_the_core() {
        let mut c = loaded();
        assert!(c.is_playing());
        c.dispatch(TransportAction::TogglePlay);
        assert_eq!(c.player().state(), PlayerState::Paused);
        assert!(c.player().engine().is_paused());
        c.dispatch(TransportAction::TogglePlay);
        assert_eq!(c.player().state(), PlayerState::Playing);
    }

    #[test]
    fn seek_actions_move_the_core_position() {
        let mut c = loaded();
        c.dispatch(TransportAction::SeekTo(45.0));
        assert!((c.player().position() - 45.0).abs() < f64::EPSILON);
        c.dispatch(TransportAction::SeekBy(-10.0));
        assert!((c.player().position() - 35.0).abs() < f64::EPSILON);
        // A relative seek never goes below zero.
        c.dispatch(TransportAction::SeekBy(-999.0));
        assert!(c.player().position().abs() < f64::EPSILON);
    }

    #[test]
    fn refused_transport_surfaces_on_the_status_line() {
        // Seeking while idle is refused by the core; the surface shows it, not swallows.
        let mut c = controller();
        assert!(c.ui().status.is_none());
        c.dispatch(TransportAction::SeekTo(5.0));
        assert!(c.ui().status.is_some(), "the refusal is surfaced honestly");
    }

    #[test]
    fn set_speed_folds_to_the_engine() {
        let mut c = loaded();
        c.dispatch(TransportAction::SetSpeed(2.0));
        assert!((c.player().controls().speed - 2.0).abs() < f64::EPSILON);
        assert!(c
            .player()
            .engine()
            .applied_control_properties()
            .contains(&("speed".to_owned(), "2".to_owned())));
    }

    #[test]
    fn ab_loop_two_marks_set_an_ordered_range_on_the_core() {
        let mut c = loaded();
        c.dispatch(TransportAction::SeekTo(30.0));
        c.dispatch(TransportAction::MarkAbLoop); // A = 30
        assert_eq!(c.ui().ab_pending, Some(30.0));
        c.dispatch(TransportAction::SeekTo(10.0));
        c.dispatch(TransportAction::MarkAbLoop); // B = 10 → ordered to (10, 30)
        assert_eq!(c.ui().ab_pending, None);
        assert_eq!(
            c.player().controls().ab_loop,
            AbLoop::Range { a: 10.0, b: 30.0 }
        );
        assert!(c
            .player()
            .engine()
            .applied_control_properties()
            .contains(&("ab-loop-a".to_owned(), "10".to_owned())));
        // Clearing folds Off back to the engine.
        c.dispatch(TransportAction::ClearAbLoop);
        assert_eq!(c.player().controls().ab_loop, AbLoop::Off);
    }

    #[test]
    fn snapshot_and_frame_step_drive_the_engine_when_playable() {
        let mut c = loaded();
        c.dispatch(TransportAction::TogglePlay); // pause first
        c.dispatch(TransportAction::FrameForward);
        c.dispatch(TransportAction::FrameBack);
        assert_eq!(c.player().engine().frame_steps(), &[true, false]);
        c.dispatch(TransportAction::Snapshot(ScreenshotMode::Video));
        assert_eq!(c.player().engine().screenshots(), &[ScreenshotMode::Video]);
        assert_eq!(c.ui().status.as_deref(), Some("Snapshot captured."));
    }

    #[test]
    fn select_track_folds_the_sid_to_the_engine() {
        let mut c = loaded();
        c.dispatch(TransportAction::SelectTrack(TrackKind::Subtitle, 1));
        assert!(c
            .player()
            .engine()
            .applied_track_properties()
            .contains(&("sid".to_owned(), "1".to_owned())));
    }

    // ── queue glue ───────────────────────────────────────────────────────────────

    #[test]
    fn enqueue_and_navigation_drive_the_core_playlist() {
        let mut c = controller();
        c.dispatch(TransportAction::Enqueue(
            "a".to_owned(),
            Some("Alpha".to_owned()),
        ));
        c.dispatch(TransportAction::Enqueue("b".to_owned(), None));
        c.dispatch(TransportAction::Enqueue("c".to_owned(), None));
        assert_eq!(c.player().playlist().len(), 3);
        // Next advances the queue and loads item 1.
        c.dispatch(TransportAction::Next);
        assert_eq!(c.player().media(), Some("b"));
        // Selecting an index makes it current + loads it.
        c.dispatch(TransportAction::SelectQueueIndex(0));
        assert_eq!(c.player().media(), Some("a"));
        // Remove + clear go through the core.
        c.dispatch(TransportAction::RemoveQueueIndex(2));
        assert_eq!(c.player().playlist().len(), 2);
        c.dispatch(TransportAction::ClearQueue);
        assert!(c.player().playlist().is_empty());
    }

    #[test]
    fn repeat_cycles_and_shuffle_toggles_on_the_core() {
        let mut c = controller();
        c.dispatch(TransportAction::Enqueue("a".to_owned(), None));
        assert_eq!(c.player().playlist().repeat(), RepeatMode::Off);
        c.dispatch(TransportAction::ToggleRepeat);
        assert_eq!(c.player().playlist().repeat(), RepeatMode::All);
        c.dispatch(TransportAction::ToggleRepeat);
        assert_eq!(c.player().playlist().repeat(), RepeatMode::One);
        c.dispatch(TransportAction::ToggleRepeat);
        assert_eq!(c.player().playlist().repeat(), RepeatMode::Off);

        assert!(!c.player().playlist().is_shuffled());
        c.dispatch(TransportAction::ToggleShuffle);
        assert!(c.player().playlist().is_shuffled());
        assert_eq!(c.player().playlist().shuffle_seed(), Some(SHUFFLE_SEED));
        c.dispatch(TransportAction::ToggleShuffle);
        assert!(!c.player().playlist().is_shuffled());
    }

    // ── library browse fold ──────────────────────────────────────────────────────

    fn library_fixture(c: &mut MediaController<FakeMpv>) {
        c.library_mut().upsert(
            "/m/beta.mp3",
            MediaMetadata::from_path("/m/beta.mp3")
                .expect("audio")
                .with_artist("Zephyr"),
        );
        c.library_mut().upsert(
            "/m/alpha.mp3",
            MediaMetadata::from_path("/m/alpha.mp3")
                .expect("audio")
                .with_artist("Aurora"),
        );
        c.library_mut().upsert(
            "/v/clip.mkv",
            MediaMetadata::from_path("/v/clip.mkv").expect("video"),
        );
    }

    #[test]
    fn visible_items_reflect_the_live_browse_query() {
        let mut c = controller();
        library_fixture(&mut c);
        // Default: all three, title-sorted (alpha, beta, clip).
        let titles: Vec<&str> = c
            .visible_items()
            .iter()
            .map(|i| i.metadata.title.as_str())
            .collect();
        assert_eq!(titles, vec!["alpha", "beta", "clip"]);
        // A search needle narrows the fold.
        c.set_search("aurora");
        let filtered: Vec<&str> = c
            .visible_items()
            .iter()
            .map(|i| i.metadata.title.as_str())
            .collect();
        assert_eq!(filtered, vec!["alpha"]);
        // Clearing the search restores everything, and the kind filter narrows it.
        c.set_search("");
        c.set_kind_filter(Some(MediaKind::Video));
        let videos: Vec<&str> = c
            .visible_items()
            .iter()
            .map(|i| i.metadata.title.as_str())
            .collect();
        assert_eq!(videos, vec!["clip"]);
    }

    #[test]
    fn sources_rows_count_items_under_each_root() {
        let mut c = controller();
        c.library_mut().add_root("/m");
        c.library_mut().add_root("/v");
        library_fixture(&mut c);
        let rows = c.sources();
        assert_eq!(rows.len(), 2);
        let m = rows.iter().find(|r| r.path == "/m").expect("/m root");
        assert_eq!(m.label, "m");
        assert_eq!(m.item_count, 2, "beta + alpha live under /m");
        let v = rows.iter().find(|r| r.path == "/v").expect("/v root");
        assert_eq!(v.item_count, 1);
    }

    // ── pure folds ───────────────────────────────────────────────────────────────

    #[test]
    fn format_time_is_minutes_then_hours() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(7.0), "0:07");
        assert_eq!(format_time(95.0), "1:35");
        assert_eq!(format_time(3725.0), "1:02:05");
        // Non-finite / negative render zero, never a panic.
        assert_eq!(format_time(-5.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    #[test]
    fn progress_fraction_clamps_and_handles_unknown_duration() {
        assert!((progress_fraction(30.0, Some(120.0)) - 0.25).abs() < f32::EPSILON);
        assert!((progress_fraction(999.0, Some(120.0)) - 1.0).abs() < f32::EPSILON);
        assert!(progress_fraction(30.0, None).abs() < f32::EPSILON);
        assert!(progress_fraction(30.0, Some(0.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn osd_shows_while_paused_or_recently_active_then_hides() {
        assert!(osd_should_show(0.0, true), "paused → always visible");
        assert!(osd_should_show(10.0, true));
        assert!(osd_should_show(1.0, false), "recent activity keeps it up");
        assert!(
            !osd_should_show(OSD_HIDE_SECS + 0.5, false),
            "idle hides it"
        );
    }

    #[test]
    fn next_repeat_cycles_off_all_one() {
        assert_eq!(next_repeat(RepeatMode::Off), RepeatMode::All);
        assert_eq!(next_repeat(RepeatMode::All), RepeatMode::One);
        assert_eq!(next_repeat(RepeatMode::One), RepeatMode::Off);
    }

    #[test]
    fn library_row_texts_join_present_metadata_only() {
        let item = LibraryItem {
            path: "/m/song.flac".to_owned(),
            metadata: MediaMetadata::from_path("/m/song.flac")
                .expect("audio")
                .with_artist("Artist")
                .with_duration(210.0),
            added_seq: 0,
        };
        let (title, subtitle) = library_row_texts(&item);
        assert_eq!(title, "song");
        assert_eq!(subtitle, "Audio · 3:30 · Artist");
    }

    #[test]
    fn track_label_reads_id_kind_and_language() {
        let t = &tracks()[1]; // the eng audio track
        assert_eq!(track_label(t), "#1 audio · eng · aac");
    }

    #[test]
    fn title_from_url_cleans_the_final_component() {
        assert_eq!(
            title_from_url("/films/Big_Buck_Bunny.mkv"),
            "Big Buck Bunny"
        );
        assert_eq!(title_from_url("http://host/stream.m3u8"), "stream");
        assert_eq!(title_from_url("plainname"), "plainname");
    }

    #[test]
    fn now_playing_title_falls_back_when_idle() {
        let c = controller();
        assert_eq!(now_playing_title(c.player()), "Nothing playing");
        let c = loaded();
        assert_eq!(now_playing_title(c.player()), "clip");
    }

    #[test]
    fn indexing_an_empty_field_reports_honestly() {
        let mut c = controller();
        c.index_current_folder();
        assert_eq!(
            c.ui().status.as_deref(),
            Some("Enter a folder path to index.")
        );
    }

    // ── Jellyfin Sources (MEDIA-10) ──────────────────────────────────────────────

    fn jelly_device() -> ClientInfo {
        ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0")
    }

    /// A Jellyfin movie with one direct-playable (mkv / h264 / aac) source.
    fn jelly_movie() -> BaseItemDto {
        BaseItemDto {
            id: "m1".into(),
            name: Some("Movie One".into()),
            item_type: Some("Movie".into()),
            media_sources: vec![MediaSourceInfo {
                id: Some("s1".into()),
                container: Some("mkv".into()),
                media_streams: vec![
                    MediaStream {
                        stream_type: Some("Video".into()),
                        codec: Some("h264".into()),
                        index: 0,
                        ..MediaStream::default()
                    },
                    MediaStream {
                        stream_type: Some("Audio".into()),
                        codec: Some("aac".into()),
                        index: 1,
                        is_default: true,
                        ..MediaStream::default()
                    },
                ],
                ..MediaSourceInfo::default()
            }],
            ..BaseItemDto::default()
        }
    }

    /// A fixture transport: `PlaybackInfo` + `/Items` serve JSON, `/Sessions` 204.
    struct StubTransport;
    impl HttpTransport for StubTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
            if request.url.contains("/Sessions/Playing") {
                return Ok(HttpResponse {
                    status: 204,
                    body: Vec::new(),
                });
            }
            let body = if request.url.contains("/PlaybackInfo") {
                r#"{"MediaSources":[{"Id":"s1","Container":"mkv","MediaStreams":[
                    {"Type":"Video","Codec":"h264","Index":0},
                    {"Type":"Audio","Codec":"aac","Index":1,"IsDefault":true}]}],
                    "PlaySessionId":"sess-1"}"#
            } else if request.url.contains("/Items") {
                r#"{"Items":[{"Id":"m1","Name":"Movie One","Type":"Movie",
                    "MediaSources":[{"Id":"s1","Container":"mkv","MediaStreams":[
                    {"Type":"Video","Codec":"h264","Index":0},
                    {"Type":"Audio","Codec":"aac","Index":1}]}]}],
                    "TotalRecordCount":1,"StartIndex":0}"#
            } else {
                "{}"
            };
            Ok(HttpResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
            })
        }
    }

    fn stub_client() -> JellyfinClient<StubTransport> {
        JellyfinClient::new("https://jelly.mesh:8096", jelly_device(), StubTransport)
            .with_auth("TOKEN", "user-1")
    }

    #[test]
    fn client_capabilities_bridge_reflects_the_mpv_baseline() {
        // The §6 bridge: the mpv baseline flows into the negotiation profile, so a
        // stock title direct-plays.
        let caps = client_capabilities(&MpvCapabilities::baseline());
        assert!(caps.supports_container("mkv"));
        assert!(caps.supports_video_codec("h264"));
        assert!(caps.supports_audio_codec("aac"));
    }

    #[test]
    fn play_jellyfin_item_negotiates_direct_play_and_opens_a_session() {
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        let decision = c.play_jellyfin_item(&jelly_movie()).expect("play");
        assert_eq!(decision.method, PlaybackMethod::DirectPlay);
        // The negotiated URL is what the core Player loaded.
        assert_eq!(c.player().media(), Some(decision.url.as_str()));
        assert!(decision.url.contains("/Videos/m1/stream?"));
        // A sync session is open, carrying the source id + default audio index.
        let report = c.jellyfin_progress_report().expect("session open");
        assert_eq!(report.item_id, "m1");
        assert_eq!(report.media_source_id.as_deref(), Some("s1"));
        assert_eq!(report.audio_stream_index, Some(1));
    }

    #[test]
    fn play_without_a_selected_server_is_refused_honestly() {
        let mut c = controller();
        let error = c.play_jellyfin_item(&jelly_movie()).expect_err("no server");
        assert!(error.contains("Select a Jellyfin server"));
        assert!(c.jellyfin_progress_report().is_none());
    }

    #[test]
    fn browse_jellyfin_materializes_items_through_the_client() {
        let mut c = controller();
        let count = c
            .browse_jellyfin(&stub_client(), &ItemsQuery::default().recursive())
            .expect("browse");
        assert_eq!(count, 1);
        assert_eq!(c.jellyfin_items().len(), 1);
        assert_eq!(c.jellyfin_items()[0].id, "m1");
    }

    #[test]
    fn open_jellyfin_item_runs_playbackinfo_then_reports_start_and_stop() {
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        let client = stub_client();
        let decision = c
            .open_jellyfin_item(
                &client,
                "https://jelly.mesh:8096",
                Some("TOKEN"),
                "m1",
                StreamMediaType::Video,
            )
            .expect("open");
        assert_eq!(decision.method, PlaybackMethod::DirectPlay);
        // The PlaySessionId from PlaybackInfo threads into the session.
        assert_eq!(decision.play_session_id.as_deref(), Some("sess-1"));
        let report = c.jellyfin_progress_report().expect("session");
        assert_eq!(report.play_session_id.as_deref(), Some("sess-1"));
        // Progress + stop both drive the client; stop clears the session.
        c.report_jellyfin_progress(&client).expect("progress");
        c.report_jellyfin_stopped(&client).expect("stop");
        assert!(c.jellyfin_progress_report().is_none());
    }

    #[test]
    fn jellyfin_sources_lists_configured_servers_with_state() {
        let mut c = controller();
        c.add_jellyfin_server("a", "Anvil", "https://a.mesh");
        c.add_jellyfin_server("b", "Backup", "https://b.mesh");
        c.select_jellyfin_server("b");
        let rows = c.jellyfin_sources();
        assert_eq!(rows.len(), 2);
        let b = rows.iter().find(|r| r.id == "b").expect("b row");
        assert!(b.selected);
        assert!(!b.signed_in);
    }

    #[test]
    fn stream_media_type_routes_music_to_the_audio_path() {
        let mut audio = jelly_movie();
        audio.item_type = Some("Audio".into());
        assert_eq!(stream_media_type(&audio), StreamMediaType::Audio);
        assert_eq!(stream_media_type(&jelly_movie()), StreamMediaType::Video);
    }

    #[test]
    fn jellyfin_item_title_falls_back_to_id() {
        let mut item = jelly_movie();
        assert_eq!(jellyfin_item_title(&item), "Movie One");
        item.name = None;
        assert_eq!(jellyfin_item_title(&item), "m1");
    }

    // ── Jellyfin offline + profiles (MEDIA-11) ───────────────────────────────────

    /// A transport that serves synthetic media bytes for a stream download and JSON
    /// for a browse — the offline download seam, no network.
    struct DownloadStub;
    impl HttpTransport for DownloadStub {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
            let body: Vec<u8> = if request.url.contains("/stream") {
                b"SYNTHETIC-OFFLINE-MEDIA".to_vec()
            } else {
                br#"{"Items":[{"Id":"m1","Name":"Movie One","Type":"Movie",
                    "MediaSources":[{"Id":"s1","Container":"mkv","MediaStreams":[
                    {"Type":"Video","Codec":"h264","Index":0},
                    {"Type":"Audio","Codec":"aac","Index":1}]}]}],
                    "TotalRecordCount":1,"StartIndex":0}"#
                    .to_vec()
            };
            Ok(HttpResponse { status: 200, body })
        }
    }

    fn download_client() -> JellyfinClient<DownloadStub> {
        JellyfinClient::new("https://jelly.mesh:8096", jelly_device(), DownloadStub)
            .with_auth("TOKEN", "user-1")
    }

    #[test]
    fn download_caches_a_title_then_plays_it_offline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        c.set_jellyfin_offline_root(dir.path());

        // Download → cache: the synthetic bytes land under the scratch root.
        let entry = c
            .download_jellyfin_item(&download_client(), &jelly_movie(), 1000)
            .expect("download");
        assert_eq!(entry.item_id, "m1");
        assert_eq!(entry.byte_len, "SYNTHETIC-OFFLINE-MEDIA".len() as u64);
        assert!(c.is_offline_available("m1"));
        // The file is really on disk with the exact bytes.
        let cached = c.jellyfin().cache().local_path("m1").expect("path");
        assert!(cached.starts_with(dir.path()));
        assert_eq!(
            std::fs::read(&cached).expect("read cached"),
            b"SYNTHETIC-OFFLINE-MEDIA"
        );

        // Offline play → the core Player loads the local file (no network).
        c.play_offline_item("m1", 2000).expect("offline play");
        assert_eq!(c.player().media(), Some(cached.to_string_lossy().as_ref()));
        // The offline list + usage reflect the one cached title.
        let rows = c.offline_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item_id, "m1");
        assert!(c.offline_usage().contains('/'));
    }

    #[test]
    fn evicting_an_offline_title_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        c.set_jellyfin_offline_root(dir.path());
        c.download_jellyfin_item(&download_client(), &jelly_movie(), 1)
            .expect("download");
        assert!(c.is_offline_available("m1"));
        c.evict_offline_item("m1");
        assert!(!c.is_offline_available("m1"));
        assert!(c.offline_rows().is_empty());
    }

    #[test]
    fn download_without_a_selected_server_is_refused_honestly() {
        let mut c = controller();
        let err = c
            .download_jellyfin_item(&download_client(), &jelly_movie(), 1)
            .expect_err("no server");
        assert!(err.contains("Select a Jellyfin server"));
        assert!(!c.is_offline_available("m1"));
    }

    #[test]
    fn play_offline_missing_title_is_refused_honestly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = controller();
        c.set_jellyfin_offline_root(dir.path());
        let err = c.play_offline_item("ghost", 1).expect_err("not cached");
        assert!(err.contains("not downloaded"));
    }

    fn auth(user_id: &str, name: &str, token: &str) -> ServerAuth {
        ServerAuth {
            access_token: token.into(),
            user_id: user_id.into(),
            user_name: Some(name.into()),
            server_id: Some("srv".into()),
        }
    }

    #[test]
    fn profiles_switch_per_server_with_token_isolation() {
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        assert!(c.add_jellyfin_profile("srv", auth("user-a", "matthew", "TOKEN-A")));
        assert!(c.add_jellyfin_profile("srv", auth("user-b", "guest", "TOKEN-B")));

        // The first profile is active; the row exposes both + the active name.
        let row = &c.jellyfin_sources()[0];
        assert!(row.signed_in);
        assert_eq!(row.active_profile.as_deref(), Some("matthew"));
        assert_eq!(row.profiles.len(), 2);
        assert!(row
            .profiles
            .iter()
            .any(|p| p.user_id == "user-a" && p.active));

        // Switching flips the active profile — and the selected server's token.
        assert!(c.switch_jellyfin_profile("srv", "user-b"));
        let row = &c.jellyfin_sources()[0];
        assert_eq!(row.active_profile.as_deref(), Some("guest"));
        assert!(row
            .profiles
            .iter()
            .any(|p| p.user_id == "user-b" && p.active));
        let active_token = c
            .jellyfin()
            .selected_server()
            .and_then(ServerConfig::active_auth)
            .map(|a| a.access_token.clone());
        assert_eq!(active_token.as_deref(), Some("TOKEN-B"));

        // Switching to an unknown profile is refused honestly.
        assert!(!c.switch_jellyfin_profile("srv", "nobody"));
        assert!(c
            .ui()
            .status
            .as_deref()
            .unwrap()
            .contains("No such profile"));
    }

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(human_bytes(15 * 1024 * 1024), "15 MB");
    }
}
