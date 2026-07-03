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

use mde_media_core::{
    AbLoop, BrowseQuery, Library, LibraryItem, MediaEngine, MediaKind, PlaybackControls, Player,
    PlayerEvent, PlayerState, Playlist, PlaylistItem, RepeatMode, ScreenshotMode, SortKey, Track,
    TrackKind, TrackSelect, TrackSelection,
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
}

/// Map a [`mde_media_core::PlayerError`] to a status string. Taken by value so the
/// call sites stay the point-free `.map_err(err)` form.
#[allow(clippy::needless_pass_by_value)]
fn err(e: mde_media_core::PlayerError) -> String {
    e.to_string()
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
}
