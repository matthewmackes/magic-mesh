//! The eframe app (E12-5): a read-only projection of the daemon-owned Music
//! workspace. Catalog and playback state arrive through retained `mde-bus`
//! snapshots; mutations are emitted only through an installed authenticated Bus
//! publisher. The legacy worker remains quarantined for removal, but no runtime
//! constructor starts it or accepts it as a fallback authority.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use mde_egui::eframe::{self, App, CreationContext};
use mde_egui::egui::{
    self, Align, Context, CursorIcon, Layout, Response, RichText, ScrollArea, Sense,
};
use mde_egui::{Motion, Style};

use mde_musicd::airsonic::{Album, Artist, Client, Song};
use mde_musicd::creds;
use mde_musicd::domain::{
    ordered_variants, select_variant, BookmarkItem, CatalogItem, ContentKind, LibraryCollection,
    MusicActionRequestV1, MusicWorkspaceSnapshotV1, PlaybackTarget, SourceVariant,
    MUSIC_CONTRACT_VERSION,
};

#[cfg(test)]
use crate::menubar::{self, MenuAction, MenuContext, NowPlaying};
use crate::model::{
    album_subtitle, format_duration, Command, Fetch, MusicState, SeatServer, Update,
};
use crate::worker;
use crate::workspace_reader::WorkspaceReader;

/// Retry the credential path while the shell is open.  Provisioning can finish
/// after the DRM shell has already been constructed, so a restart must not be
/// required just to make the newly materialized account visible.
const CREDS_RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// Bound retained daemon-state reads independently of the egui frame rate. The
/// workspace reader opens the Bus persistence store and decodes JSON, so doing
/// that on every frame turns an unrelated 60 Hz repaint into needless I/O.
const WORKSPACE_POLL_INTERVAL: Duration = Duration::from_millis(500);
static NEXT_WORKSPACE_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Keep each provider browse request small enough to cross the Bus without
/// turning a page click into an unbounded catalog transfer. The daemon's
/// current retained collection limit is larger; this is the UI page size.
const MUSIC_BROWSE_PAGE_SIZE: usize = 100;

/// Fallback snapshots remain bounded, while explicit page snapshots can move
/// through the entire provider catalog without retaining every page at once.
const MUSIC_BROWSE_MAX_RETAINED_ITEMS: usize = mde_musicd::domain::MAX_COLLECTION_ITEMS;

/// Give the daemon-owned browse lane time to replace the retained snapshot
/// before an empty selection is presented as a final provider result.
const MUSIC_DETAIL_REQUEST_GRACE: Duration = Duration::from_secs(8);

/// UI-side retained state for one daemon collection. Page-aware snapshots
/// replace this bounded window; legacy snapshots use the conservative fallback
/// inference until the next explicit browse response arrives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BrowseCollectionState {
    items: Vec<CatalogItem>,
    current_offset: usize,
    next_offset: usize,
    has_more: Option<bool>,
    requested_offset: Option<usize>,
    loading: bool,
}

#[derive(Debug, Clone)]
struct PendingDetailRequest {
    item_id: String,
    kind: ContentKind,
    started: Instant,
}

impl PendingDetailRequest {
    fn matches(&self, item: &CatalogItem) -> bool {
        self.matches_identity(&item.id, item.kind)
    }

    fn matches_identity(&self, item_id: &str, kind: ContentKind) -> bool {
        self.item_id == item_id && self.kind == kind
    }

    fn is_waiting(&self) -> bool {
        self.started.elapsed() < MUSIC_DETAIL_REQUEST_GRACE
    }
}

/// The music surface: the view-model plus the channels to its worker thread.
pub struct MusicApp {
    /// The render-agnostic state the view draws.
    state: MusicState,
    /// Outbound intents to the worker — `None` when no creds are configured yet
    /// (no worker is spawned in that case).
    commands: Option<SyncSender<Command>>,
    /// Inbound results from the worker, drained at the top of each frame.
    updates: Receiver<Update>,
    /// The configured server host, shown in the header (empty when unconfigured).
    server: String,
    /// The first-run / setup error (missing or malformed creds), if any.
    setup_error: Option<String>,
    /// Repaint handle shared with the worker and used for credential retries.
    ctx: egui::Context,
    /// Update sender retained so a worker can be started after first-run
    /// credentials appear without rebuilding the surface.
    update_tx: SyncSender<Update>,
    /// Earliest time at which a missing-credential retry may run.
    next_creds_check: Instant,
    /// Whether this instance owns the standalone Airsonic compatibility worker.
    /// Embedded shell Music is daemon-owned and must not start a competing
    /// provider/store/playback authority.
    worker_enabled: bool,
    /// Current center-pane route.
    route: MusicRoute,
    /// Current unified-library filter.
    library_filter: LibraryFilter,
    /// Selected daemon-owned Artist, Podcast, or Radio browse item.
    open_catalog_detail: Option<CatalogItem>,
    /// In-flight detail requests are identity-bound so a stale retained
    /// snapshot cannot turn a just-opened Artist, Album, or Podcast into a
    /// false "unavailable" result.
    pending_detail_request: Option<PendingDetailRequest>,
    /// Read-only daemon workspace projection for downloads/storage state.
    workspace_reader: WorkspaceReader,
    /// Earliest time at which the retained workspace snapshot may be read.
    next_workspace_poll: Instant,
    /// Optional shell-owned authenticated writer for daemon workspace actions.
    /// The standalone client leaves this unset, so it cannot present a fake
    /// mutation path or mint credentials outside the root shell authority.
    workspace_action_publisher:
        Option<Box<dyn Fn(&str) -> Result<(), String> + Send + Sync + 'static>>,
    /// Optional host-owned writer for read-only daemon browse requests. Without
    /// one, embedded and standalone surfaces report browse actions unavailable;
    /// neither may fall back to direct provider access.
    workspace_browse_publisher:
        Option<Box<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static>>,
    /// Search field state and debounce bookkeeping.
    search_query: String,
    search_generation: u64,
    search_deadline: Option<Instant>,
    /// Bounded retained pages keyed by the daemon collection key. The current
    /// snapshot hydrates this model; direct tests/integrations can call
    /// `retain_browse_page` with the same explicit page metadata.
    browse_collections: BTreeMap<String, BrowseCollectionState>,
    /// Textures decoded from daemon-local artwork paths. The UI never fetches
    /// provider URLs directly; it only decodes admitted cache files.
    artwork_textures: BTreeMap<String, egui::TextureHandle>,
    /// Artwork tokens already sent through the authenticated daemon browse
    /// lane, preventing one request per frame for a visible row.
    artwork_requests: BTreeSet<String>,
    /// Paths that failed local decode, so a corrupt cache entry falls back
    /// without repeatedly touching the filesystem.
    artwork_failures: BTreeSet<String>,
    /// Resizable/collapsible shell state.
    library_collapsed: bool,
    now_playing_open: bool,
    library_width: f32,
    now_playing_width: f32,
}

/// The content-first routes owned by the Music workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MusicRoute {
    /// Evidence-backed landing page.
    #[default]
    Home,
    /// Global catalog search.
    Search,
    /// Unified library collection.
    Library,
    /// Admitted source capabilities and renderer targets.
    Sources,
    /// Daemon-owned queue projection.
    Queue,
    /// Full-page now-playing view for narrow shells.
    NowPlaying,
    /// Album detail page.
    Album,
    /// Artist detail page.
    Artist,
    /// Podcast channel detail page.
    Podcast,
    /// Internet radio station detail page.
    Radio,
    /// Setup/authentication flow.
    Setup,
}

/// Library facets shown in the unified collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LibraryFilter {
    #[default]
    All,
    Playlists,
    Artists,
    Albums,
    Podcasts,
    Audiobooks,
    Radio,
    Downloaded,
}

impl MusicApp {
    /// Build the standalone review surface as the same daemon-projected client
    /// used by the shell. A standalone window has no signing authority, so
    /// mutations remain honestly unavailable until an authenticated publisher is
    /// installed by its host.
    #[must_use]
    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::new_with_ctx(&cc.egui_ctx)
    }

    /// Build over an egui [`egui::Context`] directly — the DRM-seat shell path
    /// (`mde-shell-egui --features drm`) has no eframe `CreationContext`, only the
    /// bare `Context` the DRM runner drives. Both entry points converge on the
    /// same daemon-projected state reader.
    #[must_use]
    pub fn new_with_ctx(ctx: &egui::Context) -> Self {
        Self::new_with_mode(ctx, false)
    }

    /// Build the embedded shell surface without starting the standalone
    /// Airsonic worker. The shell supplies authenticated daemon Bus writers and
    /// reads the retained workspace snapshot, so a local provider worker would
    /// be a competing authority rather than a useful fallback.
    #[must_use]
    pub fn new_embedded_with_ctx(ctx: &egui::Context) -> Self {
        Self::new_with_mode(ctx, false)
    }

    fn new_with_mode(ctx: &egui::Context, worker_enabled: bool) -> Self {
        let (update_tx, update_rx) =
            mpsc::sync_channel::<Update>(crate::worker::UPDATE_QUEUE_CAPACITY);
        let mut app = Self {
            state: MusicState::new(),
            commands: None,
            updates: update_rx,
            server: String::new(),
            setup_error: None,
            ctx: ctx.clone(),
            update_tx,
            next_creds_check: std::time::Instant::now(),
            worker_enabled,
            route: MusicRoute::Home,
            library_filter: LibraryFilter::All,
            open_catalog_detail: None,
            pending_detail_request: None,
            workspace_reader: WorkspaceReader::client(),
            next_workspace_poll: Instant::now(),
            workspace_action_publisher: None,
            workspace_browse_publisher: None,
            search_query: String::new(),
            search_generation: 0,
            search_deadline: None,
            browse_collections: BTreeMap::new(),
            artwork_textures: BTreeMap::new(),
            artwork_requests: BTreeSet::new(),
            artwork_failures: BTreeSet::new(),
            library_collapsed: false,
            now_playing_open: true,
            library_width: Style::MUSIC_LIBRARY_RAIL,
            now_playing_width: Style::MUSIC_NOW_PLAYING_RAIL,
        };
        if worker_enabled {
            app.try_start_with_current_creds();
        }
        app
    }

    /// Install the shell's authenticated publisher for typed daemon workspace
    /// actions. The publisher owns signing and Bus persistence; this surface
    /// only emits a validated request body.
    pub fn set_workspace_action_publisher<F>(&mut self, publisher: F)
    where
        F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    {
        self.workspace_action_publisher = Some(Box::new(publisher));
    }

    /// Install the shell's typed daemon browse writer. Browse requests are
    /// read-only Bus intents; the daemon persists the resulting catalog page
    /// and republishes it through the same retained workspace snapshot that
    /// Home and Library consume.
    pub fn set_workspace_browse_publisher<F>(&mut self, publisher: F)
    where
        F: Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
    {
        self.workspace_browse_publisher = Some(Box::new(publisher));
    }

    /// Whether this surface has crossed into daemon authority. The embedded
    /// shell starts without the local worker, and a retained snapshot or either
    /// shell-owned writer is enough to make the daemon path authoritative. In
    /// that mode a missing snapshot/writer is an unavailable state, never a
    /// reason to revive the compatibility worker's catalog or transport.
    fn daemon_authority_active(&self) -> bool {
        !self.worker_enabled
            || self.state.workspace.is_some()
            || self.workspace_action_publisher.is_some()
            || self.workspace_browse_publisher.is_some()
    }

    /// Report the connection truth that the embedded shell actually knows. The
    /// shell intentionally has no compatibility worker, so `commands.is_some()`
    /// is not a valid readiness signal for a daemon-backed Music surface.
    fn connection_status(&self) -> (&'static str, egui::Color32) {
        if let Some(snapshot) = self.state.workspace.as_ref() {
            if snapshot.any_source_reachable {
                ("Connected", Style::MUSIC_GREEN)
            } else {
                ("Source unavailable", Style::WARN)
            }
        } else if self.worker_enabled && self.commands.is_some() {
            ("Connected", Style::MUSIC_GREEN)
        } else if self.daemon_authority_active() {
            ("Connecting", Style::WARN)
        } else {
            ("Connect source", Style::WARN)
        }
    }

    fn issue_search(&mut self) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.search_generation = self.search_generation.saturating_add(1);
        self.state.search = Fetch::Loading;
        if let Some(publisher) = self.workspace_browse_publisher.as_ref() {
            let body = serde_json::json!({"query": query}).to_string();
            if let Err(error) = publisher("search", &body) {
                self.state.error = Some(format!("Music search failed: {error}"));
            }
        } else if self.daemon_authority_active() {
            // Once a retained daemon snapshot has been accepted, the legacy
            // worker must not become a second catalog authority merely because
            // the shell browse writer is temporarily unavailable.
            self.state.error = Some(
                "Music search is unavailable until the authenticated daemon browse path is connected."
                    .to_owned(),
            );
        } else {
            self.send(Command::Search {
                generation: self.search_generation,
                query,
            });
        }
    }

    fn publish_browse_request(&mut self, verb: &str, body: &str) -> bool {
        let Some(publisher) = self.workspace_browse_publisher.as_ref() else {
            if self.daemon_authority_active() {
                self.state.error = Some(format!(
                    "Music browse is unavailable until the authenticated daemon path accepts {verb}."
                ));
            }
            return false;
        };
        if let Err(error) = publisher(verb, body) {
            self.state.error = Some(format!("Music browse failed: {error}"));
            false
        } else {
            self.state.error = None;
            true
        }
    }

    /// Ask the daemon to materialize one provider artwork token. Requests are
    /// deliberately deduplicated per surface lifetime; the daemon's retained
    /// snapshot will replace the token with a local cache path when complete.
    fn request_artwork_ref(&mut self, artwork_ref: Option<&str>) {
        let Some(artwork_ref) = artwork_ref else {
            return;
        };
        if artwork_ref.starts_with('/')
            || artwork_ref.starts_with("http://")
            || artwork_ref.starts_with("https://")
            || !self.artwork_requests.insert(artwork_ref.to_owned())
        {
            return;
        }
        let body = serde_json::json!({"id": artwork_ref}).to_string();
        if !self.publish_browse_request("get-cover-art", &body) {
            self.artwork_requests.remove(artwork_ref);
        }
    }

    /// Decode one daemon-local artwork path into a cached egui texture. The
    /// image bytes never come from the provider or Bus directly, keeping the
    /// render loop bounded and making offline/stale artwork deterministic.
    fn artwork_texture(&mut self, ui: &egui::Ui, artwork_ref: &str) -> Option<egui::TextureHandle> {
        if !artwork_ref.starts_with('/') {
            return None;
        }
        if let Some(texture) = self.artwork_textures.get(artwork_ref) {
            return Some(texture.clone());
        }
        if self.artwork_failures.contains(artwork_ref) {
            return None;
        }
        let Some(bytes) = std::fs::read(artwork_ref).ok() else {
            self.artwork_failures.insert(artwork_ref.to_owned());
            return None;
        };
        let Some(decoded) = image::load_from_memory(&bytes)
            .ok()
            .map(|image| image.to_rgba8())
        else {
            self.artwork_failures.insert(artwork_ref.to_owned());
            return None;
        };
        let Ok(width) = usize::try_from(decoded.width()) else {
            self.artwork_failures.insert(artwork_ref.to_owned());
            return None;
        };
        let Ok(height) = usize::try_from(decoded.height()) else {
            self.artwork_failures.insert(artwork_ref.to_owned());
            return None;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], decoded.as_raw());
        let key = format!("music-artwork-{:016x}", artwork_ref_hash(artwork_ref));
        let texture = ui
            .ctx()
            .load_texture(key, image, egui::TextureOptions::LINEAR);
        self.artwork_textures
            .insert(artwork_ref.to_owned(), texture.clone());
        Some(texture)
    }

    /// Paint real artwork when the daemon has admitted a local path, while
    /// retaining a branded deterministic placeholder during fetch/failure.
    fn render_catalog_artwork(&mut self, ui: &mut egui::Ui, item: &CatalogItem, size: f32) {
        self.render_artwork_ref(ui, item.artwork_ref.as_deref(), &item.id, size);
    }

    fn render_artwork_ref(
        &mut self,
        ui: &mut egui::Ui,
        artwork_ref: Option<&str>,
        fallback_identity: &str,
        size: f32,
    ) {
        self.request_artwork_ref(artwork_ref);
        if let Some(artwork_ref) = artwork_ref {
            if let Some(texture) = self.artwork_texture(ui, artwork_ref) {
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        texture.id(),
                        egui::vec2(size, size),
                    ))
                    .fit_to_exact_size(egui::vec2(size, size)),
                );
                return;
            }
        }
        artwork_tile(ui, fallback_identity, size);
    }

    fn select_library_filter(&mut self, filter: LibraryFilter) {
        self.library_filter = filter;
        self.route = MusicRoute::Library;
        if let Some((key, verb)) = browse_filter_request(filter) {
            self.request_browse_page(key, verb, 0);
        }
    }

    /// Publish one bounded page request for a daemon-owned collection. The
    /// request body deliberately contains only numeric pagination fields so
    /// the authenticated shell/daemon seam can validate it independently of
    /// the UI's retained rows.
    fn request_browse_page(&mut self, collection_key: &str, verb: &str, offset: usize) {
        let offset = bounded_browse_offset(offset);
        let body = browse_request_body(offset);
        let state = self
            .browse_collections
            .entry(collection_key.to_owned())
            .or_default();
        state.requested_offset = Some(offset);
        state.loading = true;
        if !self.publish_browse_request(verb, &body) {
            if let Some(state) = self.browse_collections.get_mut(collection_key) {
                state.loading = false;
                state.requested_offset = None;
            }
        }
    }

    fn request_next_browse_page(&mut self, collection_key: &str, kind: ContentKind) {
        let Some(verb) = browse_verb_for_kind(kind) else {
            return;
        };
        let offset = self
            .browse_collections
            .get(collection_key)
            .map_or(0, |state| state.next_offset);
        self.request_browse_page(collection_key, verb, offset);
    }

    /// Retain a page-aware daemon response for a collection. Pages replace the
    /// current window so a large provider catalog stays bounded in the UI while
    /// every offset remains reachable through the next-page control.
    pub fn retain_browse_page(
        &mut self,
        collection_key: &str,
        offset: usize,
        size: usize,
        has_more: bool,
        items: Vec<CatalogItem>,
    ) {
        let offset = bounded_browse_offset(offset);
        let size = size.clamp(1, mde_musicd::domain::MAX_LIBRARY_PAGE_SIZE);
        let state = self
            .browse_collections
            .entry(collection_key.to_owned())
            .or_default();
        let page_items = items.into_iter().take(size).collect::<Vec<_>>();
        let page_len = page_items.len();
        state.items = page_items;
        state.current_offset = offset;
        state.next_offset = offset.saturating_add(page_len);
        state.has_more = Some(has_more);
        state.requested_offset = None;
        state.loading = false;
    }

    fn sync_browse_collection(&mut self, collection: &LibraryCollection) {
        let state = self
            .browse_collections
            .entry(collection.key.clone())
            .or_default();
        if collection.page_size > 0 {
            let response_matches_request = state
                .requested_offset
                .is_none_or(|offset| offset == collection.offset);
            if response_matches_request {
                state.items = collection.items.clone();
                state.current_offset = collection.offset;
                state.next_offset = collection.offset.saturating_add(collection.items.len());
                state.has_more = Some(collection.has_more);
                state.requested_offset = None;
                state.loading = false;
            }
        } else {
            merge_browse_items(&mut state.items, collection.items.iter().cloned());
            state.items.truncate(MUSIC_BROWSE_MAX_RETAINED_ITEMS);
            state.next_offset = state.items.len();
            if state.has_more.is_none() {
                state.has_more = Some(
                    state.items.len() >= MUSIC_BROWSE_PAGE_SIZE
                        && state.items.len() < MUSIC_BROWSE_MAX_RETAINED_ITEMS,
                );
            }
            if state.requested_offset.is_some_and(|offset| {
                state.items.len() > offset || state.items.len() >= MUSIC_BROWSE_MAX_RETAINED_ITEMS
            }) {
                state.requested_offset = None;
                state.loading = false;
            }
        }
    }

    fn play_bookmark(&mut self, bookmark: &BookmarkItem) {
        let request = match bookmark_play_request(bookmark) {
            Ok(request) => request,
            Err(error) => {
                self.state.error = Some(format!("Resume unavailable: {error}"));
                return;
            }
        };
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Resume is available only from the authenticated Construct shell.".to_owned());
            return;
        };
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Resume request failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    fn play_catalog_item(&mut self, item: &CatalogItem) {
        if !matches!(
            item.kind,
            ContentKind::Music
                | ContentKind::Episode
                | ContentKind::Chapter
                | ContentKind::Audiobook
        ) {
            self.state.error = Some(format!(
                "{} is browse-only: the daemon has no playable track identity for this row",
                item.title
            ));
            return;
        }
        let Some(variant) = select_variant(&item.variants) else {
            self.state.error = Some(format!(
                "{} is unavailable: no reachable or cached source variant",
                item.title
            ));
            return;
        };
        let mut request = workspace_action_request("play");
        request.content = Some(variant.content.clone());
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Catalog playback requires the authenticated Construct shell.".to_owned());
            return;
        };
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Catalog playback failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    /// Publish the station's admitted direct stream only after the user chooses
    /// the explicit detail-page action. Catalog-row activation merely opens the
    /// station detail; it never mutates playback on its own.
    fn play_radio_station(&mut self, station: &CatalogItem) {
        if self
            .state
            .workspace
            .as_ref()
            .is_some_and(|snapshot| !snapshot_retains_exact_catalog_item(snapshot, station))
        {
            self.state.error = Some(format!(
                "{} changed or was withdrawn; reopen the station from the latest Music catalog",
                station.title
            ));
            return;
        }
        let Some(variant) = admitted_radio_stream_variant(station) else {
            self.state.error = Some(format!(
                "{} is unavailable: no admitted direct HTTP stream target",
                station.title
            ));
            return;
        };
        let mut request = workspace_action_request("play");
        request.content = Some(variant.content.clone());
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Station playback requires the authenticated Construct shell.".to_owned());
            return;
        };
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Station playback failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    /// Apply the single catalog-row interaction policy shared by Home, Search,
    /// and Library. Radio is intentionally a detail route so playback always
    /// requires the explicit signed `Play station` action.
    fn activate_catalog_item(&mut self, item: &CatalogItem) {
        if is_direct_play_catalog_item(item) {
            self.play_catalog_item(item);
        } else if item.kind == ContentKind::Album {
            self.open_daemon_album(item);
        } else {
            self.open_catalog_detail(item);
        }
    }

    fn publish_download_action(&mut self, action: &str, content: mde_musicd::domain::ContentRef) {
        let mut request = workspace_action_request(action);
        request.content = Some(content);
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Download controls require the authenticated Construct shell.".to_owned());
            return;
        };
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Download action failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    fn download_catalog_item(&mut self, item: &CatalogItem) {
        if !is_downloadable_catalog_item(item) {
            self.state.error = Some(format!(
                "{} cannot be downloaded because it is live or browse-only",
                item.title
            ));
            return;
        }
        let Some(variant) = select_variant(&item.variants) else {
            self.state.error = Some(format!(
                "{} is unavailable: no reachable or cached source variant",
                item.title
            ));
            return;
        };
        self.publish_download_action("download", variant.content.clone());
    }

    fn transfer_playback_to_target(&mut self, target: &PlaybackTarget) {
        if self
            .state
            .workspace
            .as_ref()
            .is_some_and(|snapshot| !snapshot_retains_exact_playback_target(snapshot, target))
        {
            self.state.error = Some(format!(
                "{} changed or was withdrawn; choose a target from the latest Music projection",
                target.name
            ));
            return;
        }
        if !target.available {
            self.state.error = Some(format!(
                "{} is unavailable: {}",
                target.name,
                target
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("the daemon supplied no readiness reason")
            ));
            return;
        }
        if target.kind != "mesh_seat" {
            self.state.error = Some(format!(
                "{} is browse-only: this Music surface has no typed {} handoff adapter yet",
                target.name, target.kind
            ));
            return;
        }
        let mut request = workspace_action_request("transfer");
        request.target_peer = Some(target.id.clone());
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Playback handoff requires the authenticated Construct shell.".to_owned());
            return;
        };
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Playback handoff failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    /// Open one retained daemon album for detail browsing. The legacy `Album`
    /// shape is used only as a render-model bridge; track authority remains the
    /// typed MusicWorkspaceSnapshot collection rendered below.
    fn open_daemon_album(&mut self, item: &CatalogItem) {
        if item.kind != ContentKind::Album {
            self.state.error = Some(format!(
                "{} is browse-only: only retained albums have a detail view",
                item.title
            ));
            return;
        }
        self.open_catalog_detail = None;
        self.pending_detail_request = None;
        self.state.open(Album {
            id: item.id.clone(),
            name: item.title.clone(),
            artist: item.creator.clone(),
            artist_id: String::new(),
            song_count: 0,
            cover_art: item.artwork_ref.clone().unwrap_or_default(),
            year: None,
        });
        self.route = MusicRoute::Album;
        self.state.error = None;
        if let Some(variant) = select_variant(&item.variants) {
            let body = serde_json::json!({"id": variant.content.remote_id}).to_string();
            if self.publish_browse_request("get-album", &body) {
                self.pending_detail_request = Some(PendingDetailRequest {
                    item_id: item.id.clone(),
                    kind: ContentKind::Album,
                    started: Instant::now(),
                });
            }
        } else {
            self.state.error = Some(format!(
                "{} is unavailable: no reachable or cached source variant",
                item.title
            ));
        }
    }

    /// Open a non-album daemon catalog row without falling back to a local
    /// provider worker. Artist and podcast rows publish the next typed browse
    /// request; radio retains its direct stream variant for the station detail
    /// and typed playback action.
    fn open_catalog_detail(&mut self, item: &CatalogItem) {
        let route = match item.kind {
            ContentKind::Artist => MusicRoute::Artist,
            ContentKind::Podcast => MusicRoute::Podcast,
            ContentKind::Radio => MusicRoute::Radio,
            _ => return,
        };
        let variant = select_variant(&item.variants);
        self.state.close();
        self.pending_detail_request = None;
        self.open_catalog_detail = Some(item.clone());
        self.route = route;
        self.state.error = None;
        match (item.kind, variant) {
            (ContentKind::Artist, Some(variant)) => {
                let body = serde_json::json!({"id": variant.content.remote_id}).to_string();
                if self.publish_browse_request("albums-by-artist", &body) {
                    self.pending_detail_request = Some(PendingDetailRequest {
                        item_id: item.id.clone(),
                        kind: ContentKind::Artist,
                        started: Instant::now(),
                    });
                }
            }
            (ContentKind::Podcast, Some(variant)) => {
                let body = serde_json::json!({"id": variant.content.remote_id}).to_string();
                if self.publish_browse_request("podcast-episodes", &body) {
                    self.pending_detail_request = Some(PendingDetailRequest {
                        item_id: item.id.clone(),
                        kind: ContentKind::Podcast,
                        started: Instant::now(),
                    });
                }
            }
            (ContentKind::Artist | ContentKind::Podcast, None) => {
                self.state.error = Some(format!(
                    "{} is unavailable: no reachable or cached source variant",
                    item.title
                ));
            }
            (ContentKind::Radio, _) => {}
            _ => {}
        }
    }

    fn close_catalog_detail(&mut self) {
        self.open_catalog_detail = None;
        self.pending_detail_request = None;
        self.state.close();
        self.route = MusicRoute::Library;
    }

    /// Publish a daemon-owned transport action when the authenticated host
    /// writer is installed. A surface without that writer consumes the intent
    /// with an honest unavailable error so no caller can fall back to local
    /// playback authority.
    fn try_publish_transport_action(
        &mut self,
        action: &str,
        position_ms: Option<u64>,
        volume_milli: Option<u16>,
    ) -> bool {
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            if self.daemon_authority_active() {
                self.state.error = Some(
                    "Music transport is unavailable until the authenticated daemon action path is connected."
                        .to_owned(),
                );
                // Do not send this intent to the local worker after daemon
                // authority has been established.
                return true;
            }
            return false;
        };
        let mut request = workspace_action_request(action);
        request.position_ms = position_ms;
        request.volume_milli = volume_milli;
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Music action failed: {error}"));
        } else {
            self.state.error = None;
        }
        true
    }

    /// Publish a queue-relative transport or playback-policy mutation against
    /// the exact daemon queue generation currently rendered by the workspace.
    /// Next/previous, shuffle, and repeat have no compatibility-worker seam:
    /// once daemon authority is active they must never degrade into local UI
    /// state or race a replacement queue.
    fn publish_queue_playback_action(
        &mut self,
        action: &str,
        shuffle: Option<bool>,
        repeat: Option<&str>,
    ) {
        let Some(snapshot) = self.state.workspace.as_ref() else {
            self.state.error = Some("Music queue state is not available yet.".to_owned());
            return;
        };
        if snapshot.playback.current.is_none() || snapshot.queue.is_empty() {
            self.state.error = Some("Music queue has no current track.".to_owned());
            return;
        }
        let Some(publisher) = self.workspace_action_publisher.as_ref() else {
            self.state.error =
                Some("Music queue controls require the authenticated Construct shell.".to_owned());
            return;
        };

        let mut request = workspace_action_request(action);
        request.expected_queue_revision = Some(snapshot.playback.queue_revision);
        request.shuffle = shuffle;
        request.repeat = repeat.map(str::to_owned);
        if let Err(error) = publish_workspace_request(publisher.as_ref(), request) {
            self.state.error = Some(format!("Music queue action failed: {error}"));
        } else {
            self.state.error = None;
        }
    }

    /// Start the worker from the current credential file, or retain the honest
    /// first-run error until provisioning writes one.  This path is shared by
    /// initial construction and the live retry below.
    fn try_start_with_current_creds(&mut self) {
        match creds::load() {
            Ok(c) => self.start_with_creds(c),
            Err(e) => self.setup_error = Some(e.to_string()),
        }
    }

    /// Materialize the worker and kick off the first library load.
    fn start_with_creds(&mut self, creds: creds::Creds) {
        let connections = seat_connections(&creds);
        let server = connections
            .first()
            .map_or_else(String::new, |(_, client)| client.base_url().to_string());
        let servers = connections.iter().map(|(seat, _)| seat.clone()).collect();
        let commands = worker::spawn(connections, self.ctx.clone(), self.update_tx.clone());
        let _ = commands.send(Command::LoadLibrary);
        self.state.albums = Fetch::Loading;
        self.state.starred = Fetch::Loading;
        self.state.servers = servers;
        self.commands = Some(commands);
        self.server = server;
        self.setup_error = None;
        self.route = MusicRoute::Home;
        self.send(Command::LoadStarred);
    }

    /// Look for credentials that appeared after the surface was constructed.
    ///
    /// The daemon/user service and the root-owned DRM shell intentionally use
    /// the same resolver, so this is enough to converge both views without a
    /// manual shell restart.
    fn refresh_missing_credentials(&mut self) {
        if !self.worker_enabled || self.commands.is_some() {
            return;
        }
        let now = Instant::now();
        if now < self.next_creds_check {
            return;
        }
        self.next_creds_check = now + CREDS_RETRY_INTERVAL;
        self.try_start_with_current_creds();
    }

    /// Send an intent to the worker (a no-op when no worker is running).
    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.commands {
            if let Err(error) = tx.try_send(cmd) {
                if matches!(error, TrySendError::Full(_)) {
                    let _ = self.update_tx.try_send(Update::Error(
                        "Music is busy; the bounded command queue is full. Try again shortly."
                            .to_string(),
                    ));
                }
            }
        }
    }

    /// WIN7-4 — the currently loaded track, the SAME `self.state.now_playing`
    /// field the workspace reads for its own transport
    /// status cluster (no second read, §7). `mde-shell-egui`'s embedding
    /// shell holds this `MusicApp` directly (the `mde-media-egui`
    /// `MediaController::player` precedent — a thin, already-established
    /// read-only accessor shape, not a new one invented here) and reuses it
    /// for the Start Menu Music tile's live facts.
    #[must_use]
    pub fn now_playing(&self) -> Option<&Song> {
        self.state.now_playing.as_ref()
    }

    /// Snapshot the surface into the [`MenuContext`] the shared menu bar renders
    /// from (the read half of a frame) — its connection health, the transport
    /// state, whether an album is open, and the now-playing readout. The elapsed
    /// playhead is clamped to the tagged length so a slightly-ahead poll never
    /// reads past the end; a `0` duration (a stream the server gave no length for)
    /// leaves the total off. The bar never reaches into the surface mid-render, so
    /// its gating + status cluster stay unit-testable without egui.
    #[cfg(test)]
    fn menu_context(&self) -> MenuContext {
        let now_playing = self.state.now_playing.as_ref().map(|song| {
            let duration_secs = u64::from(song.duration);
            let elapsed_secs = self.state.position_ms / 1000;
            NowPlaying {
                title: song.title.clone(),
                artist: song.artist.clone(),
                elapsed_secs: if duration_secs > 0 {
                    elapsed_secs.min(duration_secs)
                } else {
                    elapsed_secs
                },
                duration_secs,
            }
        });
        MenuContext {
            connected: self.commands.is_some(),
            library_failed: matches!(self.state.albums, Fetch::Failed(_)),
            has_track: self.state.now_playing.is_some(),
            playing: self.state.playing,
            album_open: self.state.open_album.is_some(),
            server: self.server.clone(),
            now_playing,
        }
    }

    /// Dispatch a menu-bar [`MenuAction`] to its real seam (§6, one dispatch path).
    /// The transport + library-reload actions become the worker [`Command`] they
    /// map to; Reload Album resolves the open album's id from live state; Back to
    /// Library is a local navigation seam ([`MusicState::close`]) with no worker
    /// round-trip. No new behaviour — every arm drives an existing seam.
    #[cfg(test)]
    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::ReloadAlbum => {
                if let Some(open) = &self.state.open_album {
                    let id = open.album.id.clone();
                    self.send(Command::LoadAlbum(id));
                }
            }
            MenuAction::BackToLibrary => self.state.close(),
            other => {
                if let Some(cmd) = other.command() {
                    self.send(cmd);
                }
            }
        }
    }

    /// The album library listing (or its loading/empty/error state).
    fn render_library(&mut self, ui: &mut egui::Ui) {
        render_library_filter_bar(ui, self);
        ui.add_space(Style::SP_S);
        if self.library_filter == LibraryFilter::Downloaded {
            self.render_downloads(ui);
            return;
        }
        if let Some(snapshot) = self.state.workspace.clone() {
            self.render_daemon_library(ui, &snapshot);
            return;
        }
        if self.daemon_authority_active() {
            state_card(
                ui,
                "Waiting for Music daemon state",
                "The retained workspace snapshot is not available yet.",
                Style::TEXT_DIM,
            );
            return;
        }
        let mut to_open: Option<Album> = None;
        match &self.state.albums {
            Fetch::Idle | Fetch::Loading => {
                centered_state(ui, true, "Loading library…");
            }
            Fetch::Failed(e) => {
                ui.colored_label(Style::DANGER, format!("Couldn't load the library: {e}"));
            }
            Fetch::Cached(albums) if albums.is_empty() => {
                centered_state(
                    ui,
                    false,
                    "The music server is offline and the cached library is empty.",
                );
            }
            Fetch::Cached(albums) => {
                ui.colored_label(Style::TEXT_DIM, "Offline — showing cached library");
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for album in albums {
                            if album_row(ui, album).clicked() {
                                to_open = Some(album.clone());
                            }
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            Fetch::Ready(albums) if albums.is_empty() => {
                centered_state(ui, false, "This server has no albums yet.");
            }
            Fetch::Ready(albums) => {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for album in albums {
                            if album_row(ui, album).clicked() {
                                to_open = Some(album.clone());
                            }
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
        }

        if let Some(album) = to_open {
            let id = album.id.clone();
            self.state.open(album);
            self.send(Command::LoadAlbum(id));
        }
    }

    /// Render the daemon's bounded collections when its retained workspace
    /// projection is available. This is the authority cut for Library on every
    /// public construction path.
    fn render_daemon_library(&mut self, ui: &mut egui::Ui, snapshot: &MusicWorkspaceSnapshotV1) {
        let wanted_kind = match self.library_filter {
            LibraryFilter::All => None,
            LibraryFilter::Playlists => Some(ContentKind::Playlist),
            LibraryFilter::Artists => Some(ContentKind::Artist),
            LibraryFilter::Albums => Some(ContentKind::Album),
            LibraryFilter::Podcasts => Some(ContentKind::Podcast),
            LibraryFilter::Audiobooks => Some(ContentKind::Audiobook),
            LibraryFilter::Radio => Some(ContentKind::Radio),
            LibraryFilter::Downloaded => unreachable!("downloads are rendered before filtering"),
        };
        let collections: Vec<LibraryCollection> = snapshot
            .collections
            .iter()
            .filter(|collection| wanted_kind.is_none_or(|kind| collection.kind == kind))
            .cloned()
            .collect();
        if collections.is_empty() {
            let message = if snapshot.any_source_reachable {
                "No items in this daemon collection yet."
            } else {
                "No admitted music source is reachable; cached daemon rows are also empty."
            };
            state_card(ui, "Library unavailable", message, Style::WARN);
            return;
        }
        for collection in collections {
            self.render_daemon_collection(ui, &collection);
        }
    }

    fn render_daemon_collection(&mut self, ui: &mut egui::Ui, collection: &LibraryCollection) {
        self.sync_browse_collection(collection);
        let key = collection.key.clone();
        let title = collection.title.clone();
        let kind = collection.kind;
        let (items, has_more, loading) = self.browse_collections.get(&key).map_or_else(
            || (Vec::new(), false, false),
            |state| {
                (
                    state.items.clone(),
                    state.has_more.unwrap_or(false),
                    state.loading,
                )
            },
        );

        ui.label(Style::music_title(&title));
        ui.add_space(Style::SP_S);
        ScrollArea::vertical()
            .id_salt(format!("daemon-library-{key}"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in &items {
                    if self.daemon_catalog_row(ui, item).clicked() {
                        self.activate_catalog_item(item);
                    }
                    if is_downloadable_catalog_item(item) {
                        let enabled = self.workspace_action_publisher.is_some();
                        let button = ui.add_enabled(enabled, egui::Button::new("Download"));
                        if button.clicked() {
                            self.download_catalog_item(item);
                        }
                        if !enabled {
                            let _ = mde_egui::disabled_hover_text(button,
                                "Download requires the authenticated Construct shell action path.",
                            );
                        }
                    }
                    ui.add_space(Style::SP_XS);
                }
            });

        if has_more {
            let can_request = !loading
                && browse_verb_for_kind(kind).is_some()
                && self.workspace_browse_publisher.is_some();
            let label = if loading { "Loading…" } else { "Load more" };
            let button = ui.add_enabled(can_request, egui::Button::new(label));
            if button.clicked() {
                self.request_next_browse_page(&key, kind);
            }
            if !can_request && !loading {
                let _ = mde_egui::disabled_hover_text(button,
                    "Load more requires the authenticated Construct shell browse path.",
                );
            }
        }
        ui.add_space(Style::SP_M);
    }

    /// Render the daemon-owned offline catalogue and cache budget. This path is
    /// intentionally read-only: controls that mutate downloads remain typed
    /// Bus actions and are not reimplemented by the legacy worker surface.
    fn render_downloads(&mut self, ui: &mut egui::Ui) {
        let Some(snapshot) = self.state.workspace.clone() else {
            state_card(
                ui,
                "Waiting for Music daemon state",
                "The retained workspace snapshot is not available yet.",
                Style::TEXT_DIM,
            );
            return;
        };

        let storage = snapshot.storage;
        let used = storage.used_bytes.min(storage.cap_bytes);
        #[allow(clippy::cast_precision_loss)]
        let ratio = (used as f32 / storage.cap_bytes as f32).clamp(0.0, 1.0);
        ui.label(Style::music_title("Downloads"));
        ui.label(
            RichText::new(format!(
                "{} used of {}",
                format_bytes(storage.used_bytes),
                format_bytes(storage.cap_bytes),
            ))
            .color(Style::TEXT_DIM),
        );
        ui.add(egui::ProgressBar::new(ratio).text(format_bytes(used)));
        ui.add_space(Style::SP_M);

        if snapshot.downloads.is_empty() {
            state_card(
                ui,
                "No managed downloads",
                "Pin finite audio from an admitted source to keep it offline.",
                Style::TEXT_DIM,
            );
            return;
        }
        for record in &snapshot.downloads {
            let content = record.content.clone();
            let action = match record.state.as_str() {
                "ready" if record.pinned => Some(("unpin_download", "Unpin")),
                "ready" => Some(("pin_download", "Pin")),
                "queued" | "downloading" => Some(("cancel_download", "Cancel")),
                "failed" | "cancelled" => Some(("remove_download", "Remove")),
                _ => None,
            };
            egui::Frame::NONE
                .fill(Style::MUSIC_SURFACE)
                .inner_margin(Style::SP_S as i8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(&record.content.remote_id)
                                .color(Style::TEXT_STRONG)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{:?} · {}{}",
                                record.content.kind,
                                record.state,
                                if record.pinned { " · pinned" } else { "" },
                            ))
                            .small()
                            .color(Style::TEXT_DIM),
                        );
                    });
                    if let Some(total) = record.total_bytes {
                        #[allow(clippy::cast_precision_loss)]
                        let progress = (record.bytes as f32 / total as f32).clamp(0.0, 1.0);
                        ui.add(egui::ProgressBar::new(progress).text(format!(
                            "{} / {}",
                            format_bytes(record.bytes),
                            format_bytes(total)
                        )));
                    } else {
                        ui.label(
                            RichText::new(format_bytes(record.bytes))
                                .small()
                                .color(Style::TEXT_DIM),
                        );
                    }
                    if let Some(error) = &record.error_code {
                        ui.colored_label(Style::DANGER, error);
                    }
                    if let Some((action, label)) = action {
                        let enabled = self.workspace_action_publisher.is_some();
                        let button = ui.add_enabled(enabled, egui::Button::new(label));
                        if button.clicked() {
                            self.publish_download_action(action, content.clone());
                        }
                        if !enabled {
                            let _ = mde_egui::disabled_hover_text(button,
                                "Download controls require the authenticated Construct shell action path.",
                            );
                        }
                    }
                });
            ui.add_space(Style::SP_XS);
        }
    }

    /// The open album's header + track list (or its loading/empty/error state).
    fn render_album(&mut self, ui: &mut egui::Ui) {
        if self.daemon_authority_active() {
            if self.state.workspace.is_some() {
                self.render_daemon_album(ui);
            } else {
                state_card(
                    ui,
                    "Waiting for Music daemon state",
                    "The retained workspace snapshot is not available yet.",
                    Style::TEXT_DIM,
                );
            }
            return;
        }
        let mut go_back = false;
        let mut to_play: Option<Song> = None;

        if let Some(open) = &self.state.open_album {
            // The shared Music MenuBar owns workspace identity. Album title and
            // return navigation are domain content below it, so this view does
            // not create a second host title strip in either runner.
            ui.horizontal(|ui| {
                if ui.button("‹ Library").clicked() {
                    go_back = true;
                }
                ui.label(
                    RichText::new(&open.album.name)
                        .size(Style::TYPE_TITLE3)
                        .color(Style::TEXT),
                );
            });
            let subtitle = album_subtitle(&open.album);
            if !subtitle.is_empty() {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(subtitle)
                        .size(Style::BODY)
                        .color(Style::TEXT_DIM),
                );
            }
            ui.add_space(Style::SP_S);

            match &open.tracks {
                Fetch::Idle | Fetch::Loading => {
                    centered_state(ui, true, "Loading tracks…");
                }
                Fetch::Failed(e) => {
                    ui.colored_label(Style::DANGER, format!("Couldn't load tracks: {e}"));
                }
                Fetch::Cached(songs) => {
                    ui.colored_label(Style::TEXT_DIM, "Offline — showing cached tracks");
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, song) in songs.iter().enumerate() {
                                if track_row(ui, i, song).clicked() {
                                    to_play = Some(song.clone());
                                }
                                ui.add_space(Style::SP_XS);
                            }
                        });
                }
                Fetch::Ready(songs) if songs.is_empty() => {
                    centered_state(ui, false, "This album has no tracks.");
                }
                Fetch::Ready(songs) => {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, song) in songs.iter().enumerate() {
                                if track_row(ui, i, song).clicked() {
                                    to_play = Some(song.clone());
                                }
                                // Same inter-row breathing room the album listing
                                // gives its rows, so both lists share one rhythm.
                                ui.add_space(Style::SP_XS);
                            }
                        });
                }
            }
        }

        if go_back {
            self.state.close();
        }
        if let Some(song) = to_play {
            self.send(Command::Play(song));
        }
    }

    /// Render an album from the daemon's retained album/song collections. A
    /// missing song collection is an honest unavailable state; it must not
    /// trigger an Airsonic fetch behind the embedded surface.
    fn render_daemon_album(&mut self, ui: &mut egui::Ui) {
        let Some(open) = self.state.open_album.as_ref() else {
            state_card(
                ui,
                "Album unavailable",
                "The retained daemon album selection is no longer available.",
                Style::WARN,
            );
            return;
        };
        let album_name = open.album.name.clone();
        let album_artist = open.album.artist.clone();
        let album_artwork = open.album.cover_art.clone();
        let album_id = open.album.id.clone();
        let snapshot = self.state.workspace.clone();
        let mut go_back = false;

        ui.horizontal(|ui| {
            if ui.button("‹ Library").clicked() {
                go_back = true;
            }
            ui.label(
                RichText::new(&album_name)
                    .size(Style::TYPE_TITLE3)
                    .color(Style::TEXT),
            );
        });
        if !album_artist.is_empty() {
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(&album_artist).color(Style::TEXT_DIM));
        }
        ui.add_space(Style::SP_S);
        self.render_artwork_ref(
            ui,
            (!album_artwork.is_empty()).then_some(album_artwork.as_str()),
            &album_name,
            Style::MUSIC_HERO_ART_NARROW,
        );
        ui.add_space(Style::SP_S);

        let songs = snapshot
            .as_ref()
            .into_iter()
            .flat_map(|snapshot| snapshot.collections.iter())
            .filter(|collection| collection.kind == ContentKind::Music)
            .flat_map(|collection| collection.items.iter())
            .filter(|item| {
                item.parent_title == album_name
                    && (album_artist.is_empty() || item.creator == album_artist)
            })
            .take(mde_musicd::domain::MAX_COLLECTION_ITEMS)
            .cloned()
            .collect::<Vec<_>>();

        if songs.is_empty() {
            let request_waiting = self.pending_detail_request.as_ref().is_some_and(|pending| {
                pending.matches_identity(&album_id, ContentKind::Album) && pending.is_waiting()
            });
            if request_waiting {
                state_card(
                    ui,
                    "Loading tracks…",
                    "The daemon is retrieving this album from the music provider.",
                    Style::TEXT_DIM,
                );
                ui.ctx().request_repaint_after(WORKSPACE_POLL_INTERVAL);
                if go_back {
                    self.pending_detail_request = None;
                    self.state.close();
                    self.route = MusicRoute::Library;
                }
                return;
            }
            self.pending_detail_request = None;
            let message = snapshot.as_ref().map_or(
                "The retained daemon snapshot is unavailable.",
                |snapshot| {
                    if snapshot.any_source_reachable {
                        "This retained album has no songs in the current library window."
                    } else {
                        "No admitted source is reachable and no cached album tracks are retained."
                    }
                },
            );
            state_card(ui, "Album unavailable", message, Style::WARN);
        } else {
            self.pending_detail_request = None;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for item in &songs {
                        if self.daemon_catalog_row(ui, item).clicked() {
                            self.play_catalog_item(item);
                        }
                        let enabled = self.workspace_action_publisher.is_some();
                        let button = ui.add_enabled(enabled, egui::Button::new("Download"));
                        if button.clicked() {
                            self.download_catalog_item(item);
                        }
                        if !enabled {
                            let _ = mde_egui::disabled_hover_text(button,
                                "Download requires the authenticated Construct shell action path.",
                            );
                        }
                        ui.add_space(Style::SP_XS);
                    }
                });
        }

        if go_back {
            self.state.close();
            self.route = MusicRoute::Library;
        }
    }

    /// Render the next layer for the daemon-owned Artist and Podcast hubs, or
    /// the honest capability state for a Radio station. The rows come only
    /// from the retained typed snapshot; no detail view calls Airsonic from
    /// the GUI.
    fn render_catalog_detail(&mut self, ui: &mut egui::Ui) {
        let Some(detail) = self.open_catalog_detail.clone() else {
            state_card(
                ui,
                "Browse detail unavailable",
                "The selected daemon catalog row is no longer retained.",
                Style::WARN,
            );
            return;
        };
        let mut go_back = false;
        ui.horizontal(|ui| {
            if ui.button("‹ Library").clicked() {
                go_back = true;
            }
            ui.label(
                RichText::new(&detail.title)
                    .size(Style::TYPE_TITLE3)
                    .color(Style::TEXT),
            );
        });
        if detail.kind == ContentKind::Radio {
            ui.add_space(Style::SP_S);
            ui.label(RichText::new("Internet radio station").color(Style::TEXT_DIM));
            let stream_available = admitted_radio_stream_variant(&detail).is_some();
            if stream_available {
                let enabled = self.workspace_action_publisher.is_some();
                let button = ui.add_enabled(enabled, egui::Button::new("Play station"));
                if button.clicked() {
                    self.play_radio_station(&detail);
                }
                if !enabled {
                    let _ = mde_egui::disabled_hover_text(button,
                        "Playback requires the authenticated Construct shell action path.",
                    );
                }
                ui.label(
                    RichText::new("Direct stream target admitted")
                        .small()
                        .color(Style::TEXT_DIM),
                );
            }
            state_card(
                ui,
                if stream_available {
                    "Station admitted"
                } else {
                    "Station unavailable"
                },
                if stream_available {
                    "The daemon has the station identity and direct stream target."
                } else {
                    "The daemon has the station identity, but no usable stream target was returned."
                },
                Style::TEXT_DIM,
            );
        } else {
            let snapshot = self.state.workspace.clone();
            let items = snapshot
                .as_ref()
                .into_iter()
                .flat_map(|snapshot| snapshot.collections.iter())
                .filter(|collection| match detail.kind {
                    ContentKind::Artist => collection.kind == ContentKind::Album,
                    ContentKind::Podcast => collection.kind == ContentKind::Episode,
                    _ => false,
                })
                .flat_map(|collection| collection.items.iter())
                .filter(|item| match detail.kind {
                    ContentKind::Artist => item.creator == detail.title,
                    ContentKind::Podcast => item.parent_title == detail.title,
                    _ => false,
                })
                .take(mde_musicd::domain::MAX_COLLECTION_ITEMS)
                .cloned()
                .collect::<Vec<_>>();
            if items.is_empty() {
                let request_waiting = self
                    .pending_detail_request
                    .as_ref()
                    .is_some_and(|pending| pending.matches(&detail) && pending.is_waiting());
                if request_waiting {
                    let label = match detail.kind {
                        ContentKind::Artist => "Loading albums…",
                        ContentKind::Podcast => "Loading episodes…",
                        _ => "Loading details…",
                    };
                    state_card(
                        ui,
                        label,
                        "The daemon is retrieving this selection from the music provider.",
                        Style::TEXT_DIM,
                    );
                    ui.ctx().request_repaint_after(WORKSPACE_POLL_INTERVAL);
                    if go_back {
                        self.close_catalog_detail();
                    }
                    return;
                }
                self.pending_detail_request = None;
                let message = if snapshot.is_some_and(|snapshot| snapshot.any_source_reachable) {
                    "The provider returned no retained detail rows for this selection yet."
                } else {
                    "No admitted source is reachable and no cached detail rows are retained."
                };
                state_card(ui, "No detail rows", message, Style::TEXT_DIM);
            } else {
                self.pending_detail_request = None;
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for item in &items {
                            if self.daemon_catalog_row(ui, item).clicked() {
                                if detail.kind == ContentKind::Artist {
                                    self.open_daemon_album(item);
                                } else {
                                    self.play_catalog_item(item);
                                }
                            }
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
        }
        if go_back {
            self.close_catalog_detail();
        }
    }
}

fn bounded_browse_offset(offset: usize) -> usize {
    offset.min(mde_musicd::domain::MAX_LIBRARY_OFFSET)
}

fn browse_request_body(offset: usize) -> String {
    serde_json::json!({
        "offset": bounded_browse_offset(offset),
        "size": MUSIC_BROWSE_PAGE_SIZE,
    })
    .to_string()
}

fn browse_verb_for_kind(kind: ContentKind) -> Option<&'static str> {
    match kind {
        ContentKind::Album => Some("list-albums"),
        ContentKind::Artist => Some("list-artists"),
        ContentKind::Podcast => Some("list-podcasts"),
        ContentKind::Radio => Some("list-radio"),
        ContentKind::Playlist
        | ContentKind::Music
        | ContentKind::Episode
        | ContentKind::Audiobook
        | ContentKind::Chapter => None,
    }
}

fn browse_filter_request(filter: LibraryFilter) -> Option<(&'static str, &'static str)> {
    match filter {
        LibraryFilter::Artists => Some(("artists", "list-artists")),
        LibraryFilter::Podcasts => Some(("podcasts", "list-podcasts")),
        LibraryFilter::Radio => Some(("radio", "list-radio")),
        LibraryFilter::Albums => Some(("albums", "list-albums")),
        LibraryFilter::All
        | LibraryFilter::Playlists
        | LibraryFilter::Audiobooks
        | LibraryFilter::Downloaded => None,
    }
}

fn merge_browse_items<I>(target: &mut Vec<CatalogItem>, incoming: I)
where
    I: IntoIterator<Item = CatalogItem>,
{
    for item in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.id == item.id && existing.kind == item.kind)
        {
            *existing = item;
        } else if target.len() < MUSIC_BROWSE_MAX_RETAINED_ITEMS {
            target.push(item);
        }
    }
}

/// Build the per-seat candidate set without changing the shared credential
/// contract. `MDE_MUSIC_SEATS` is an optional operator-owned list of
/// `seat|http(s)-url|priority` entries separated by `;`; the stored server is
/// always retained as the local/default candidate.
fn seat_connections(c: &mde_musicd::creds::Creds) -> Vec<(SeatServer, Client)> {
    let mut specs = vec![(
        SeatServer::new("local", c.server_url.clone()),
        c.server_url.clone(),
    )];
    if let Ok(raw) = std::env::var("MDE_MUSIC_SEATS") {
        for item in raw.split(';').filter(|item| !item.trim().is_empty()) {
            let mut fields = item.split('|');
            let (Some(seat), Some(url)) = (fields.next(), fields.next()) else {
                continue;
            };
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let mut server = SeatServer::new(seat.trim(), url.trim());
            server.operator_priority = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            specs.push((server, url.trim().to_string()));
        }
    }
    specs
        .into_iter()
        .map(|(mut server, url)| {
            if server.seat == "local" {
                server.operator_priority = 0;
            }
            (server, Client::new(url, c.username.clone(), &c.password))
        })
        .collect()
}

/// Render the music surface's central content into the given `ui`.
///
/// Draws the honest daemon-unavailable state before a retained projection
/// arrives, then renders the daemon-owned workspace. Mutating clicks require a
/// host-installed authenticated Bus publisher.
///
/// This is the one body shared by the standalone binary's `CentralPanel` and the
/// embedded shell panel (E12-3b), so the surface renders identically whether it
/// owns a window or is a panel inside the one shell — the EMBED model of E12
/// "Construct" §5 (surfaces are panels in the shell, not separate clients). It draws
/// only through the shared [`Style`], reusing `app`'s existing state (no parallel
/// state is introduced).
#[cfg(test)]
fn music_panel(ui: &mut egui::Ui, app: &mut MusicApp) {
    app.render_workspace_content(ui);
}

/// Render the complete self-contained Music workspace: top navigation, flexible
/// content, optional rails, and the persistent player. The shell and standalone
/// entry points both mount this one function, so geometry and state presentation
/// cannot drift between them.
pub fn music_workspace(ui: &mut egui::Ui, app: &mut MusicApp) {
    let width = ui.available_width();
    let narrow = width <= 1100.0;
    let collapsed = app.library_collapsed || (width > 1100.0 && width < 1280.0);

    render_workspace_topbar(ui, app, narrow);
    if narrow {
        render_compact_workspace_nav(ui, app);
    }
    ui.add_space(Style::MUSIC_GUTTER);

    if narrow {
        app.now_playing_open = false;
        ui.vertical(|ui| app.render_workspace_content(ui));
    } else {
        ui.horizontal(|ui| {
            render_library_rail(ui, app, collapsed);
            ui.add_space(Style::MUSIC_GUTTER);
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width());
                app.render_workspace_content(ui);
            });
            if app.now_playing_open && width >= 1280.0 {
                ui.add_space(Style::MUSIC_GUTTER);
                render_now_playing_rail(ui, app);
            }
        });
    }
    render_bottom_player(ui, app, narrow);
}

/// Drain the worker's updates into the surface state — the per-frame **state
/// pump**.
///
/// The standalone [`MusicApp`]'s `update` calls this at the top of every frame;
/// the E12 shell (E12-3b) calls it for the mounted surface each frame too, because
/// the shell owns the one frame loop and never calls the surface's `App::update`.
/// Non-blocking (`try_recv`) and a no-op when the worker has sent nothing — or when
/// no worker is running (the unconfigured, no-creds surface).
pub fn music_pump(app: &mut MusicApp) {
    app.refresh_missing_credentials();
    if app.worker_enabled && app.commands.is_none() {
        // An otherwise idle first-run surface still needs frames to notice a
        // credential file written by provisioning.
        app.ctx.request_repaint_after(CREDS_RETRY_INTERVAL);
    }
    if app.search_deadline.is_some() {
        app.ctx.request_repaint_after(Duration::from_millis(250));
    }
    if app.worker_enabled {
        while let Ok(update) = app.updates.try_recv() {
            app.state.apply(update);
        }
    } else {
        // The embedded surface owns no worker. Drain any stale compatibility
        // messages without letting them become a second playback/store state.
        while app.updates.try_recv().is_ok() {}
    }
    let now = Instant::now();
    if now >= app.next_workspace_poll {
        app.next_workspace_poll = now + WORKSPACE_POLL_INTERVAL;
        if let Some(snapshot) = app.workspace_reader.poll() {
            app.state.apply_workspace_snapshot(snapshot);
        }
    }
    if app.daemon_authority_active() {
        let now = Instant::now();
        app.ctx
            .request_repaint_after(app.next_workspace_poll.saturating_duration_since(now));
    }
}

/// Render the surface's **shared top menu bar** (MENUBAR-ALL) into `ui`, then
/// dispatch the action the operator picked to its real seam.
///
/// The bar carries the UPPERCASE `MUSIC` title, the Playback / Library / View
/// menus, and the live status cluster (server health + now-playing). The standalone
/// app frames it in the window's top panel; the E12 shell renders it above the
/// mounted [`music_panel`] so the embedded surface keeps the same discoverable
/// chrome + transport the standalone binary shows. The bar stays out of
/// [`music_panel`] because the shell supplies its own surrounding chrome. Takes
/// `&mut` because Back to Library mutates the surface's own view state (§6 glue: the
/// menu is the mouse twin of an existing seam).
#[cfg(test)]
fn music_header(ui: &mut egui::Ui, app: &mut MusicApp) {
    let cx = app.menu_context();
    if let Some(action) = menubar::show(ui, &cx) {
        app.run_menu_action(action);
    }
}

impl MusicApp {
    fn render_workspace_content(&mut self, ui: &mut egui::Ui) {
        if let Some(detail) = self.setup_error.clone() {
            self.route = MusicRoute::Setup;
            render_setup_workspace(ui, &detail);
            return;
        }

        if self.state.servers.len() > 1 {
            let servers = self.state.servers.clone();
            ui.horizontal(|ui| {
                ui.label(Style::music_body("Source"));
                for server in servers {
                    let selected = self
                        .state
                        .selected_server
                        .as_ref()
                        .is_some_and(|current| current.seat == server.seat);
                    if ui.selectable_label(selected, &server.seat).clicked() {
                        self.send(Command::SelectServer(server.seat));
                    }
                }
            });
            ui.add_space(Style::SP_S);
        }
        if let Some(request) = self.state.failover.clone() {
            mde_egui::card().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        Style::MUSIC_GREEN,
                        format!("{} → {}", request.from, request.to),
                    );
                    ui.label(Style::music_body(request.reason));
                    if ui.button("Approve").clicked() {
                        self.send(Command::ApproveFailover);
                    }
                    if ui.button("Keep current").clicked() {
                        self.send(Command::RejectFailover);
                        self.state.failover = None;
                    }
                });
            });
            ui.add_space(Style::SP_S);
        }
        if let Some(error) = self.state.error.clone() {
            ui.colored_label(Style::DANGER, error);
            ui.add_space(Style::SP_S);
        }

        match self.route {
            MusicRoute::Search => render_search_results(ui, self),
            MusicRoute::Album => self.render_album(ui),
            MusicRoute::Artist | MusicRoute::Podcast | MusicRoute::Radio => {
                self.render_catalog_detail(ui)
            }
            MusicRoute::Library => {
                ui.horizontal(|ui| {
                    ui.label(Style::music_title("Your Library"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Recently added").color(Style::TEXT_DIM));
                    });
                });
                ui.add_space(Style::SP_M);
                self.render_library(ui);
            }
            MusicRoute::Sources => render_sources_page(ui, self),
            MusicRoute::Queue => render_daemon_queue_page(ui, self),
            MusicRoute::NowPlaying => render_now_playing_rail(ui, self),
            _ if self.state.open_album.is_some() => self.render_album(ui),
            _ => render_home(ui, self),
        }
    }
}

fn render_compact_workspace_nav(ui: &mut egui::Ui, app: &mut MusicApp) {
    egui::Frame::NONE
        .fill(Style::MUSIC_SURFACE)
        .inner_margin(Style::SP_XS as i8)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, route) in [
                    ("Home", MusicRoute::Home),
                    ("Search", MusicRoute::Search),
                    ("Library", MusicRoute::Library),
                    ("Now Playing", MusicRoute::NowPlaying),
                    ("Queue", MusicRoute::Queue),
                    ("Sources", MusicRoute::Sources),
                ] {
                    if ui.selectable_label(app.route == route, label).clicked() {
                        app.route = route;
                    }
                }
            });
        });
}

fn render_workspace_topbar(ui: &mut egui::Ui, app: &mut MusicApp, narrow: bool) {
    let frame = egui::Frame::NONE
        .fill(Style::MUSIC_SURFACE)
        .inner_margin(egui::Margin::symmetric(
            Style::SP_S as i8,
            Style::SP_XS as i8,
        ));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            if mde_egui::hover_text(ui.button("‹"), "Back").clicked() {
                if matches!(
                    app.route,
                    MusicRoute::Album
                        | MusicRoute::Artist
                        | MusicRoute::Podcast
                        | MusicRoute::Radio
                ) {
                    app.close_catalog_detail();
                } else {
                    app.route = MusicRoute::Home;
                    app.state.close();
                }
            }
            if mde_egui::hover_text(ui.button("›"), "Forward").clicked() {
                app.route = MusicRoute::Library;
            }
            ui.separator();
            ui.label(Style::music_title("Music"));
            if !narrow {
                ui.add_space(Style::SP_S);
            }
            let response = ui.add_sized(
                [
                    if narrow {
                        Style::MUSIC_HERO_ART_NARROW
                    } else {
                        Style::MUSIC_LIBRARY_RAIL
                    },
                    Style::CONTROL_H_M,
                ],
                egui::TextEdit::singleline(&mut app.search_query)
                    .hint_text("Search music, podcasts, audiobooks"),
            );
            if response.changed() {
                app.route = MusicRoute::Search;
                app.search_deadline = Some(Instant::now() + Duration::from_millis(250));
            }
            if mde_egui::hover_text(ui.button("⌕"), "Search").clicked() {
                app.route = MusicRoute::Search;
                app.search_deadline = Some(Instant::now());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button(if narrow {
                        "Now Playing"
                    } else if app.now_playing_open {
                        "Hide Now Playing"
                    } else {
                        "Now Playing"
                    })
                    .clicked()
                {
                    if narrow {
                        app.route = MusicRoute::NowPlaying;
                    } else {
                        app.now_playing_open = !app.now_playing_open;
                    }
                }
                let (status, status_color) = app.connection_status();
                ui.colored_label(status_color, format!("● {status}"));
            });
        });
    });
    if app
        .search_deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        app.search_deadline = None;
        if !app.search_query.trim().is_empty() {
            app.issue_search();
        }
    }
}

fn render_library_rail(ui: &mut egui::Ui, app: &mut MusicApp, collapsed: bool) {
    let width = if collapsed {
        Style::MUSIC_LIBRARY_COLLAPSED
    } else {
        app.library_width
    };
    egui::Frame::NONE
        .fill(Style::MUSIC_SURFACE)
        .inner_margin(Style::SP_S as i8)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width - Style::SP_M);
                if ui
                    .button(if collapsed { "→" } else { "← Collapse" })
                    .clicked()
                {
                    app.library_collapsed = !app.library_collapsed;
                }
                ui.add_space(Style::SP_S);
                let entries = [
                    ("⌂", "Home", MusicRoute::Home),
                    ("⌕", "Search", MusicRoute::Search),
                    ("▤", "Your Library", MusicRoute::Library),
                    ("▶", "Now Playing", MusicRoute::NowPlaying),
                    ("≡", "Queue", MusicRoute::Queue),
                    ("◌", "Sources", MusicRoute::Sources),
                ];
                for (icon, label, route) in entries {
                    let text = if collapsed {
                        icon.to_string()
                    } else {
                        format!("{icon}  {label}")
                    };
                    if ui.selectable_label(app.route == route, text).clicked() {
                        app.route = route;
                    }
                }
                if !collapsed {
                    ui.add_space(Style::SP_M);
                    ui.label(RichText::new("YOUR LIBRARY").small().color(Style::TEXT_DIM));
                    for (label, filter) in [
                        ("Playlists", LibraryFilter::Playlists),
                        ("Artists", LibraryFilter::Artists),
                        ("Albums", LibraryFilter::Albums),
                        ("Podcasts", LibraryFilter::Podcasts),
                        ("Audiobooks", LibraryFilter::Audiobooks),
                        ("Radio", LibraryFilter::Radio),
                        ("Downloaded", LibraryFilter::Downloaded),
                    ] {
                        if ui
                            .selectable_label(
                                app.library_filter == filter && app.route == MusicRoute::Library,
                                label,
                            )
                            .clicked()
                        {
                            app.select_library_filter(filter);
                        }
                    }
                }
            });
        });
}

fn render_library_filter_bar(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Browse").small().color(Style::TEXT_DIM));
        for (label, filter) in [
            ("All", LibraryFilter::All),
            ("Playlists", LibraryFilter::Playlists),
            ("Artists", LibraryFilter::Artists),
            ("Albums", LibraryFilter::Albums),
            ("Podcasts", LibraryFilter::Podcasts),
            ("Audiobooks", LibraryFilter::Audiobooks),
            ("Radio", LibraryFilter::Radio),
            ("Downloaded", LibraryFilter::Downloaded),
        ] {
            if ui
                .selectable_label(app.library_filter == filter, label)
                .clicked()
            {
                app.select_library_filter(filter);
            }
        }
    });
}

/// Render the source/capability plane from the same retained snapshot as Home
/// and Library. This makes a connected daemon visibly complete even when there
/// is only one source and no legacy worker-side server list.
fn render_sources_page(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.label(Style::music_title("Sources"));
    ui.label(RichText::new("Admitted music providers and playback targets").color(Style::TEXT_DIM));
    ui.add_space(Style::SP_M);

    let Some(snapshot) = app.state.workspace.as_ref() else {
        state_card(
            ui,
            "Waiting for Music daemon state",
            "Source capabilities will appear when the retained workspace projection arrives.",
            Style::TEXT_DIM,
        );
        return;
    };
    let sources = snapshot.sources.clone();
    let reachable = snapshot.any_source_reachable;
    if sources.is_empty() {
        state_card(
            ui,
            if reachable {
                "Source identity unavailable"
            } else {
                "No admitted sources"
            },
            "The daemon has not retained a source capability record for this seat.",
            Style::WARN,
        );
    } else {
        for source in sources {
            egui::Frame::NONE
                .fill(Style::MUSIC_SURFACE)
                .inner_margin(Style::SP_S as i8)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&source.source_id).strong().color(Style::TEXT));
                        ui.label(
                            RichText::new(if source.reachable {
                                "Connected"
                            } else {
                                "Unavailable"
                            })
                            .small()
                            .color(if source.reachable {
                                Style::MUSIC_GREEN
                            } else {
                                Style::WARN
                            }),
                        );
                        ui.label(
                            RichText::new(&source.api_profile)
                                .small()
                                .color(Style::TEXT_DIM),
                        );
                    });
                    if source.authentication_required {
                        ui.label(
                            RichText::new("Authentication required")
                                .small()
                                .color(Style::WARN),
                        );
                    }
                    let features = source.features.iter().take(16).cloned().collect::<Vec<_>>();
                    if !features.is_empty() {
                        ui.label(
                            RichText::new(format!("Features: {}", features.join(" · ")))
                                .small()
                                .color(Style::TEXT_DIM),
                        );
                    }
                });
            ui.add_space(Style::SP_XS);
        }
    }

    render_daemon_targets(ui, app);
}

fn render_daemon_queue_page(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.label(Style::music_title("Queue"));
    ui.label(RichText::new("Daemon-owned order and current-track state").color(Style::TEXT_DIM));
    ui.add_space(Style::SP_M);
    render_daemon_queue(ui, app);
}

fn render_home(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(Style::music_title("Good evening"));
            ui.label(
                RichText::new("Your music, gathered from admitted sources").color(Style::TEXT_DIM),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Open Library").clicked() {
                app.route = MusicRoute::Library;
            }
        });
    });
    ui.add_space(Style::SP_L);
    if let Some(song) = app.state.cached_track.clone() {
        ui.label(Style::music_title("Continue Listening"));
        ui.add_space(Style::SP_S);
        track_summary_card(ui, &song, app.state.playing, app);
        ui.add_space(Style::SP_L);
    }
    let bookmarks = app
        .state
        .workspace
        .as_ref()
        .map(|snapshot| snapshot.bookmarks.clone())
        .unwrap_or_default();
    if !bookmarks.is_empty() {
        render_bookmark_shelf(ui, &bookmarks, app);
        ui.add_space(Style::SP_L);
    }
    if let Some(snapshot) = app.state.workspace.as_ref() {
        let shelves = snapshot.shelves.clone();
        if shelves.is_empty() {
            let message = if snapshot.any_source_reachable {
                "No admitted source has supplied a Home shelf yet."
            } else {
                "No admitted music source is reachable and no retained Home shelf is available."
            };
            state_card(ui, "Music catalog unavailable", message, Style::WARN);
        } else {
            for shelf in &shelves {
                render_daemon_catalog_shelf(ui, shelf, app);
                ui.add_space(Style::SP_L);
            }
        }
        // An accepted daemon snapshot is the catalog authority even when it is
        // empty. Do not fall through to the compatibility worker's albums.
        return;
    }

    if app.daemon_authority_active() {
        state_card(
            ui,
            "Waiting for Music daemon state",
            "The retained workspace snapshot is not available yet.",
            Style::TEXT_DIM,
        );
        return;
    }

    if let Fetch::Ready(starred) = &app.state.starred {
        let starred = starred.clone();
        render_album_shelf(ui, "Starred", &starred, app);
        ui.add_space(Style::SP_L);
    }
    {
        ui.label(Style::music_title("Recently Added"));
        ui.add_space(Style::SP_S);
        let albums_state = app.state.albums.clone();
        match albums_state {
            Fetch::Idle | Fetch::Loading => skeleton_shelf(ui),
            Fetch::Failed(error) => state_card(ui, "Source unavailable", &error, Style::DANGER),
            Fetch::Cached(albums) => {
                state_card(
                    ui,
                    "Offline library",
                    "Showing the last cached catalog",
                    Style::WARN,
                );
                render_album_cards(ui, &albums, app);
            }
            Fetch::Ready(albums) if albums.is_empty() => state_card(
                ui,
                "Library is empty",
                "Add music to an admitted source to see it here",
                Style::TEXT_DIM,
            ),
            Fetch::Ready(albums) => render_album_cards(ui, &albums, app),
        }
    }
}

fn render_daemon_catalog_shelf(
    ui: &mut egui::Ui,
    shelf: &mde_musicd::domain::HomeShelf,
    app: &mut MusicApp,
) {
    ui.label(Style::music_title(&shelf.title));
    ui.add_space(Style::SP_S);
    ScrollArea::horizontal()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for item in shelf.items.iter().take(12) {
                    let response = egui::Frame::NONE
                        .fill(Style::MUSIC_SURFACE)
                        .inner_margin(Style::SP_S as i8)
                        .show(ui, |ui| {
                            ui.set_min_width(Style::MUSIC_HERO_ART_NARROW);
                            app.render_catalog_artwork(ui, item, Style::MUSIC_HERO_ART_NARROW);
                            ui.label(RichText::new(&item.title).color(Style::TEXT).strong());
                            if !item.creator.is_empty() {
                                ui.label(
                                    RichText::new(&item.creator).color(Style::TEXT_DIM).small(),
                                );
                            }
                            if item.cached {
                                ui.label(RichText::new("Cached").color(Style::MUSIC_GREEN).small());
                            }
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_cursor(CursorIcon::PointingHand);
                    if response.clicked() {
                        app.activate_catalog_item(item);
                    }
                    ui.add_space(Style::SP_M);
                }
            });
        });
}

fn is_direct_play_catalog_item(item: &CatalogItem) -> bool {
    matches!(
        item.kind,
        ContentKind::Music | ContentKind::Episode | ContentKind::Chapter | ContentKind::Audiobook
    )
}

fn is_well_formed_direct_stream(value: &str) -> bool {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(|character| character.is_whitespace())
    {
        return false;
    }
    let Some(rest) = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest
        .split(|character| matches!(character, '/' | '?' | '#'))
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return false;
    }
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        let Some(close) = bracketed.find(']') else {
            return false;
        };
        let suffix = &bracketed[close + 1..];
        if !suffix.is_empty()
            && !suffix.strip_prefix(':').is_some_and(|port| {
                !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return false;
        }
        &bracketed[..close]
    } else if let Some((host, port)) = host_port.rsplit_once(':') {
        if host.contains(':') || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return false;
        }
        host
    } else {
        host_port
    };
    !host.is_empty() && host.chars().any(|character| character.is_alphanumeric())
}

fn admitted_radio_stream_variant(station: &CatalogItem) -> Option<&SourceVariant> {
    (station.kind == ContentKind::Radio)
        .then(|| ordered_variants(&station.variants))?
        .into_iter()
        .find(|variant| {
            variant.content.kind == ContentKind::Radio
                && !variant.content.source_id.trim().is_empty()
                && is_well_formed_direct_stream(&variant.content.remote_id)
        })
}

/// Confirm that a retained detail row is still the exact daemon-admitted item.
/// Exact duplicates across Home/Search/Library are harmless, while any
/// conflicting binding for the same UI identity fails closed.
fn snapshot_retains_exact_catalog_item(
    snapshot: &MusicWorkspaceSnapshotV1,
    selected: &CatalogItem,
) -> bool {
    let mut retained = false;
    let candidates = snapshot
        .shelves
        .iter()
        .flat_map(|shelf| shelf.items.iter())
        .chain(
            snapshot
                .collections
                .iter()
                .flat_map(|collection| collection.items.iter()),
        )
        .chain(
            snapshot
                .search
                .iter()
                .flat_map(|page| page.groups.values())
                .flat_map(|items| items.iter()),
        );
    for candidate in candidates {
        if candidate.id == selected.id && candidate.kind == selected.kind {
            if candidate != selected {
                return false;
            }
            retained = true;
        }
    }
    retained
}

/// Bind a handoff action to the exact target generation rendered by the
/// current daemon projection. A restarted daemon may reuse a peer id while
/// changing its kind, readiness, or display identity; retained UI state must
/// not turn that replacement into authority for an old transfer action.
fn snapshot_retains_exact_playback_target(
    snapshot: &MusicWorkspaceSnapshotV1,
    selected: &PlaybackTarget,
) -> bool {
    let mut retained = false;
    for candidate in snapshot
        .targets
        .iter()
        .filter(|candidate| candidate.id == selected.id)
    {
        if candidate != selected {
            return false;
        }
        retained = true;
    }
    retained
}

fn is_downloadable_catalog_item(item: &CatalogItem) -> bool {
    matches!(
        item.kind,
        ContentKind::Music | ContentKind::Episode | ContentKind::Chapter | ContentKind::Audiobook
    )
}

impl MusicApp {
    fn daemon_catalog_row(&mut self, ui: &mut egui::Ui, item: &CatalogItem) -> Response {
        let direct_play = is_direct_play_catalog_item(item);
        let navigable = direct_play
            || matches!(
                item.kind,
                ContentKind::Album
                    | ContentKind::Artist
                    | ContentKind::Podcast
                    | ContentKind::Radio
            );
        egui::Frame::NONE
            .fill(Style::MUSIC_SURFACE)
            .inner_margin(Style::SP_S as i8)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    self.render_catalog_artwork(ui, item, Style::ICON_L);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&item.title).color(Style::TEXT).strong());
                        if !item.creator.is_empty() {
                            ui.label(RichText::new(&item.creator).color(Style::TEXT_DIM));
                        }
                        if !item.parent_title.is_empty() {
                            ui.label(
                                RichText::new(&item.parent_title)
                                    .color(Style::TEXT_DIM)
                                    .small(),
                            );
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if item.cached {
                            ui.label(RichText::new("Cached").color(Style::MUSIC_GREEN).small());
                        }
                        if item.kind == ContentKind::Radio {
                            ui.label(
                                RichText::new("Station details")
                                    .color(Style::TEXT_DIM)
                                    .small(),
                            );
                        } else if !direct_play {
                            ui.label(RichText::new("Browse-only").color(Style::TEXT_DIM).small());
                        }
                    });
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(if navigable {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Default
            })
    }
}

/// Render the daemon's typed provider bookmarks without creating a second
/// bookmark or playback authority in the legacy worker surface. Episode,
/// chapter, and audiobook rows remain honest resume metadata until their typed
/// daemon playback action is available to this UI.
fn render_bookmark_shelf(ui: &mut egui::Ui, bookmarks: &[BookmarkItem], app: &mut MusicApp) {
    ui.label(Style::music_title("Resume"));
    ui.add_space(Style::SP_S);
    for bookmark in bookmarks.iter().take(12) {
        egui::Frame::NONE
            .fill(Style::MUSIC_SURFACE)
            .inner_margin(Style::SP_S as i8)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    app.render_artwork_ref(
                        ui,
                        bookmark.artwork_ref.as_deref(),
                        &bookmark.content.remote_id,
                        Style::ICON_L,
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&bookmark.title).color(Style::TEXT).strong());
                        let subtitle =
                            match (bookmark.creator.as_str(), bookmark.parent_title.as_str()) {
                                (creator, parent) if !creator.is_empty() && !parent.is_empty() => {
                                    format!("{creator} · {parent}")
                                }
                                (creator, _) if !creator.is_empty() => creator.to_string(),
                                (_, parent) if !parent.is_empty() => parent.to_string(),
                                _ => "Provider bookmark".to_string(),
                            };
                        ui.label(RichText::new(subtitle).color(Style::TEXT_DIM).small());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let position = format_duration(bookmark.position_ms / 1000);
                        let progress = bookmark
                            .duration_ms
                            .filter(|duration| *duration > 0)
                            .map_or(0.0, |duration| {
                                (bookmark.position_ms as f32 / duration as f32).clamp(0.0, 1.0)
                            });
                        ui.vertical(|ui| {
                            ui.label(RichText::new(position).color(Style::TEXT_DIM).small());
                            if bookmark.duration_ms.is_some() {
                                ui.add(egui::ProgressBar::new(progress).desired_width(96.0));
                            }
                        });
                    });
                    let resume_enabled = app.workspace_action_publisher.is_some();
                    let button = ui.add_enabled(resume_enabled, egui::Button::new("Resume"));
                    if button.clicked() {
                        app.play_bookmark(bookmark);
                    }
                    if !resume_enabled {
                        let _ = mde_egui::disabled_hover_text(button,
                            "Resume requires the authenticated Construct shell action path.",
                        );
                    }
                });
            });
        ui.add_space(Style::SP_XS);
    }
}

fn workspace_action_request(action: &str) -> MusicActionRequestV1 {
    MusicActionRequestV1 {
        schema_version: MUSIC_CONTRACT_VERSION,
        request_id: format!(
            "music-ui-{action}-{}",
            NEXT_WORKSPACE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
        ),
        action: action.to_owned(),
        content: None,
        expected_queue_revision: None,
        position_ms: None,
        volume_milli: None,
        shuffle: None,
        repeat: None,
        queue_index: None,
        target_queue_index: None,
        target_peer: None,
        playlist: None,
        playlist_name: None,
        playlist_song_ids: Vec::new(),
        playlist_remove_indices: Vec::new(),
        armed_token: None,
    }
}

/// Build the exact typed request emitted by a Resume shelf row. Signing and
/// persistence remain outside this crate in the shell; no token is retained in
/// the UI or serialized into this unsigned body.
fn bookmark_play_request(bookmark: &BookmarkItem) -> Result<MusicActionRequestV1, String> {
    let mut request = workspace_action_request("play");
    request.content = Some(bookmark.content.clone());
    request.position_ms = Some(bookmark.position_ms);
    request
        .validate()
        .map_err(|error| format!("invalid bookmark request: {error}"))?;
    Ok(request)
}

fn publish_workspace_request(
    publisher: &(dyn Fn(&str) -> Result<(), String> + Send + Sync + 'static),
    request: MusicActionRequestV1,
) -> Result<(), String> {
    request
        .validate()
        .map_err(|error| format!("invalid Music action: {error}"))?;
    let body = serde_json::to_string(&request)
        .map_err(|error| format!("serialize Music action: {error}"))?;
    publisher(&body)
}

fn render_album_shelf(ui: &mut egui::Ui, title: &str, albums: &[Album], app: &mut MusicApp) {
    ui.label(Style::music_title(title));
    ui.add_space(Style::SP_S);
    render_album_cards(ui, albums, app);
}

fn render_album_cards(ui: &mut egui::Ui, albums: &[Album], app: &mut MusicApp) {
    ScrollArea::horizontal()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for album in albums.iter().take(12) {
                    let response = egui::Frame::NONE
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                artwork_tile(ui, &album.id, Style::MUSIC_HERO_ART_NARROW);
                                ui.label(RichText::new(&album.name).color(Style::TEXT).strong());
                                ui.label(
                                    RichText::new(&album.artist).color(Style::TEXT_DIM).small(),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_cursor(CursorIcon::PointingHand);
                    if response.clicked() {
                        app.state.open(album.clone());
                        app.route = MusicRoute::Album;
                        app.send(Command::LoadAlbum(album.id.clone()));
                    }
                    ui.add_space(Style::SP_M);
                }
            });
        });
}

fn render_search_results(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.label(Style::music_title("Search results"));
    ui.add_space(Style::SP_S);
    if app.daemon_authority_active() {
        let daemon_page = app
            .state
            .workspace
            .as_ref()
            .and_then(|snapshot| snapshot.search.clone());
        if let Some(page) = daemon_page.filter(|page| page.query == app.search_query.trim()) {
            render_daemon_search_page(ui, &page, app);
        } else if app.state.workspace.is_none() {
            state_card(
                ui,
                "Waiting for Music daemon state",
                "The retained workspace snapshot is not available yet.",
                Style::TEXT_DIM,
            );
        } else if !app.search_query.trim().is_empty() {
            skeleton_shelf(ui);
        } else {
            state_card(
                ui,
                "Search every admitted catalog",
                "Type a title, creator, podcast, or audiobook",
                Style::TEXT_DIM,
            );
        }
        return;
    }
    match &app.state.search {
        Fetch::Idle => state_card(
            ui,
            "Search every admitted catalog",
            "Type a title, creator, podcast, or audiobook",
            Style::TEXT_DIM,
        ),
        Fetch::Loading => skeleton_shelf(ui),
        Fetch::Failed(error) => state_card(ui, "Search unavailable", &error, Style::DANGER),
        Fetch::Cached(_) => state_card(
            ui,
            "Offline search",
            "Showing only cached matches",
            Style::WARN,
        ),
        Fetch::Ready(results) => {
            let result = results.clone();
            render_search_group(ui, "Artists", &result.artists, app);
            render_search_album_group(ui, "Albums", &result.albums, app);
            render_search_song_group(ui, "Tracks", &result.songs, app);
            if result.artists.is_empty() && result.albums.is_empty() && result.songs.is_empty() {
                state_card(
                    ui,
                    "No matches",
                    "Try a broader title or creator",
                    Style::TEXT_DIM,
                );
            }
        }
    }
}

fn render_daemon_search_page(
    ui: &mut egui::Ui,
    page: &mde_musicd::domain::SearchPage,
    app: &mut MusicApp,
) {
    if page.groups.is_empty() {
        state_card(
            ui,
            "No matches",
            "Try a broader title or creator",
            Style::TEXT_DIM,
        );
        return;
    }
    for (kind, items) in &page.groups {
        ui.label(
            RichText::new(format!("{kind:?}"))
                .size(Style::TYPE_HEADLINE)
                .color(Style::TEXT_STRONG),
        );
        for item in items.iter().take(mde_musicd::domain::MAX_SEARCH_ITEMS) {
            if app.daemon_catalog_row(ui, item).clicked() {
                app.activate_catalog_item(item);
            }
            ui.add_space(Style::SP_XS);
        }
        ui.add_space(Style::SP_M);
    }
}

fn render_search_group(ui: &mut egui::Ui, title: &str, artists: &[Artist], _app: &mut MusicApp) {
    if artists.is_empty() {
        return;
    }
    ui.label(
        RichText::new(title)
            .size(Style::TYPE_HEADLINE)
            .color(Style::TEXT_STRONG),
    );
    for artist in artists.iter().take(8) {
        ui.horizontal(|ui| {
            artwork_tile(ui, &artist.id, Style::ICON_L);
            ui.label(RichText::new(&artist.name).color(Style::TEXT));
        });
    }
    ui.add_space(Style::SP_M);
}

fn render_search_album_group(ui: &mut egui::Ui, title: &str, albums: &[Album], app: &mut MusicApp) {
    if albums.is_empty() {
        return;
    }
    ui.label(
        RichText::new(title)
            .size(Style::TYPE_HEADLINE)
            .color(Style::TEXT_STRONG),
    );
    render_album_cards(ui, albums, app);
    ui.add_space(Style::SP_M);
}

fn render_search_song_group(ui: &mut egui::Ui, title: &str, songs: &[Song], app: &mut MusicApp) {
    if songs.is_empty() {
        return;
    }
    ui.label(
        RichText::new(title)
            .size(Style::TYPE_HEADLINE)
            .color(Style::TEXT_STRONG),
    );
    for song in songs.iter().take(12) {
        if track_row(ui, 0, song).clicked() {
            app.send(Command::Play(song.clone()));
        }
    }
}

fn render_now_playing_rail(ui: &mut egui::Ui, app: &mut MusicApp) {
    egui::Frame::NONE
        .fill(Style::MUSIC_SURFACE)
        .inner_margin(Style::SP_M as i8)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(app.now_playing_width - Style::SP_XL);
                ui.label(RichText::new("NOW PLAYING").small().color(Style::TEXT_DIM));
                ui.add_space(Style::SP_M);
                if let Some(song) = app.state.now_playing.clone() {
                    app.render_artwork_ref(
                        ui,
                        (!song.cover_art.is_empty()).then_some(song.cover_art.as_str()),
                        &song.id,
                        Style::MUSIC_HERO_ART_WIDE,
                    );
                    ui.add_space(Style::SP_M);
                    ui.label(
                        RichText::new(&song.title)
                            .size(Style::TYPE_TITLE2)
                            .color(Style::TEXT_STRONG),
                    );
                    ui.label(RichText::new(&song.artist).color(Style::TEXT_DIM));
                    ui.add_space(Style::SP_L);
                    render_daemon_queue(ui, app);
                    render_daemon_targets(ui, app);
                    ui.add_space(Style::SP_L);
                    ui.label(
                        RichText::new("Lyrics / transcript")
                            .size(Style::TYPE_HEADLINE)
                            .color(Style::TEXT_STRONG),
                    );
                    ui.label(
                        RichText::new("Not supplied by this source")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                } else {
                    state_card(
                        ui,
                        "Nothing playing",
                        "Choose a track to see artwork and queue state",
                        Style::TEXT_DIM,
                    );
                    if app.state.workspace.is_some() {
                        ui.add_space(Style::SP_L);
                        render_daemon_queue(ui, app);
                        render_daemon_targets(ui, app);
                    }
                }
            });
        });
}

/// Render the daemon's bounded queue projection without inventing local queue
/// state or emitting an unsupported mutation. Queue identity and currentness
/// come from the same retained workspace snapshot used by the shell reader.
fn render_daemon_queue(ui: &mut egui::Ui, app: &mut MusicApp) {
    ui.label(
        RichText::new("Queue")
            .size(Style::TYPE_HEADLINE)
            .color(Style::TEXT_STRONG),
    );
    let Some(snapshot) = app.state.workspace.clone() else {
        ui.label(
            RichText::new("Queue state is not available from the daemon yet.")
                .small()
                .color(Style::TEXT_DIM),
        );
        return;
    };
    if snapshot.queue.is_empty() {
        ui.label(
            RichText::new("Queue is empty")
                .small()
                .color(Style::TEXT_DIM),
        );
        return;
    }
    for (index, entry) in snapshot.queue.iter().take(32).enumerate() {
        let current = snapshot
            .playback
            .current
            .as_ref()
            .is_some_and(|content| content == &entry.content);
        let title = if entry.title.trim().is_empty() {
            entry.content.remote_id.as_str()
        } else {
            entry.title.as_str()
        };
        let label = if current {
            format!("▶ {}. {}", index + 1, title)
        } else {
            format!("{}. {}", index + 1, title)
        };
        ui.horizontal(|ui| {
            let artwork = snapshot
                .collections
                .iter()
                .flat_map(|collection| collection.items.iter())
                .find(|item| {
                    item.variants
                        .iter()
                        .any(|variant| variant.content == entry.content)
                })
                .and_then(|item| item.artwork_ref.as_deref());
            app.render_artwork_ref(ui, artwork, &entry.content.remote_id, Style::ICON_M);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).color(if current {
                    Style::TEXT_STRONG
                } else {
                    Style::TEXT_DIM
                }));
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        entry.content.source_id,
                        format!("{:?}", entry.content.kind)
                    ))
                    .small()
                    .color(Style::TEXT_DIM),
                );
            });
        });
    }
    if snapshot.queue.len() > 32 {
        ui.label(
            RichText::new(format!("{} more queue entries", snapshot.queue.len() - 32))
                .small()
                .color(Style::TEXT_DIM),
        );
    }
}

/// Render only the bounded targets the daemon can prove. Mesh-seat targets
/// expose typed handoff; DLNA/local targets remain visible with an honest
/// adapter-unavailable label until their control path is implemented.
fn render_daemon_targets(ui: &mut egui::Ui, app: &mut MusicApp) {
    let Some(targets) = app.state.workspace.as_ref().map(|snapshot| {
        snapshot
            .targets
            .iter()
            .take(16)
            .cloned()
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    if targets.is_empty() {
        return;
    }
    ui.add_space(Style::SP_M);
    ui.label(
        RichText::new("Playback targets")
            .size(Style::TYPE_HEADLINE)
            .color(Style::TEXT_STRONG),
    );
    for target in targets {
        let can_handoff = target.available
            && target.kind == "mesh_seat"
            && app.workspace_action_publisher.is_some();
        let mut send_clicked = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&target.name).color(Style::TEXT));
            ui.label(
                RichText::new(&target.kind)
                    .small()
                    .color(Style::TEXT_DIM),
            );
            if let Some(reason) = target.unavailable_reason.as_deref() {
                ui.label(RichText::new(reason).small().color(Style::WARN));
            }
            let button = ui.add_enabled(can_handoff, egui::Button::new("Send"));
            if button.clicked() {
                send_clicked = true;
            }
            if !can_handoff {
                let _ = mde_egui::disabled_hover_text(button,
                    "Handoff requires an available mesh seat and the authenticated Construct shell action path.",
                );
            }
        });
        if send_clicked {
            app.transfer_playback_to_target(&target);
        }
    }
}

fn render_bottom_player(ui: &mut egui::Ui, app: &mut MusicApp, narrow: bool) {
    egui::TopBottomPanel::bottom("music-player").show_inside(ui, |ui| {
        egui::Frame::NONE
            .fill(Style::MUSIC_SURFACE)
            .inner_margin(Style::SP_S as i8)
            .show(ui, |ui| {
                if let Some(song) = app.state.now_playing.clone() {
                    ui.horizontal(|ui| {
                        app.render_artwork_ref(
                            ui,
                            (!song.cover_art.is_empty()).then_some(song.cover_art.as_str()),
                            &song.id,
                            if narrow {
                                Style::ICON_L
                            } else {
                                Style::ICON_XL
                            },
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&song.title)
                                    .color(Style::TEXT_STRONG)
                                    .strong(),
                            );
                            ui.label(RichText::new(&song.artist).small().color(Style::TEXT_DIM));
                        });
                        if mde_egui::hover_text(ui.button("⏮"), "Previous track").clicked() {
                            if app.state.workspace.is_some() {
                                app.publish_queue_playback_action("previous", None, None);
                            } else if !app.try_publish_transport_action("seek", Some(0), None) {
                                app.send(Command::Seek(0));
                            }
                        }
                        let label = if app.state.playing { "⏸" } else { "▶" };
                        if ui.button(label).clicked() {
                            let action = if app.state.playing { "pause" } else { "resume" };
                            if !app.try_publish_transport_action(action, None, None) {
                                app.send(if app.state.playing {
                                    Command::Pause
                                } else {
                                    Command::Resume
                                });
                            }
                        }
                        if ui.button("⏹").clicked() {
                            if !app.try_publish_transport_action("stop", None, None) {
                                app.send(Command::Stop);
                            }
                        }
                        if app.state.workspace.is_some()
                            && mde_egui::hover_text(ui.button("⏭"), "Next track").clicked()
                        {
                            app.publish_queue_playback_action("next", None, None);
                        }
                        if !narrow {
                            let duration_ms = u64::from(song.duration).saturating_mul(1000);
                            if duration_ms > 0 {
                                let mut position = app.state.position_ms.min(duration_ms);
                                if ui
                                    .add(
                                        egui::Slider::new(&mut position, 0..=duration_ms)
                                            .show_value(false),
                                    )
                                    .changed()
                                {
                                    if !app.try_publish_transport_action(
                                        "seek",
                                        Some(position),
                                        None,
                                    ) {
                                        app.send(Command::Seek(position));
                                    }
                                }
                                ui.label(format_duration(position / 1000));
                                ui.label(format_duration(duration_ms / 1000));
                            }
                            let mut volume = app
                                .state
                                .volume_milli
                                .map_or(1.0, |volume| f32::from(volume) / 1000.0);
                            if ui
                                .add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false))
                                .changed()
                            {
                                let volume_milli = (volume.clamp(0.0, 1.0) * 1000.0).round() as u16;
                                if !app.try_publish_transport_action(
                                    "set_volume",
                                    None,
                                    Some(volume_milli),
                                ) {
                                    app.send(Command::SetVolume(volume));
                                }
                            }
                            if let Some(playback) = app
                                .state
                                .workspace
                                .as_ref()
                                .map(|snapshot| snapshot.playback.clone())
                            {
                                let shuffle_label = if playback.shuffle {
                                    "Shuffle on"
                                } else {
                                    "Shuffle off"
                                };
                                if mde_egui::hover_text(
                                    ui.selectable_label(playback.shuffle, "🔀"),
                                    shuffle_label,
                                )
                                .clicked()
                                {
                                    app.publish_queue_playback_action(
                                        "shuffle",
                                        Some(!playback.shuffle),
                                        None,
                                    );
                                }
                                let next_repeat = match playback.repeat.as_str() {
                                    "off" => "context",
                                    "context" => "track",
                                    _ => "off",
                                };
                                if mde_egui::hover_text(
                                    ui.button(match playback.repeat.as_str() {
                                        "track" => "🔂",
                                        _ => "🔁",
                                    }),
                                    format!(
                                        "Repeat {} (choose {next_repeat})",
                                        playback.repeat
                                    ),
                                )
                                .clicked()
                                {
                                    app.publish_queue_playback_action(
                                        "repeat",
                                        None,
                                        Some(next_repeat),
                                    );
                                }
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("Local seat").small().color(Style::TEXT_DIM));
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Nothing playing").color(Style::TEXT_DIM));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new("Connect a source to start listening")
                                    .small()
                                    .color(Style::TEXT_DIM),
                            );
                        });
                    });
                }
            });
    });
}

fn render_setup_workspace(ui: &mut egui::Ui, detail: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_XL);
        artwork_tile(ui, "connect-music-source", Style::ICON_XL * 2.0);
        ui.add_space(Style::SP_M);
        ui.label(Style::music_title("Connect a music source"));
        ui.label(RichText::new("Music needs an admitted Subsonic/OpenSubsonic source.").color(Style::TEXT_DIM));
        ui.add_space(Style::SP_S);
        ui.label(RichText::new(detail).small().color(Style::TEXT_DIM));
        ui.add_space(Style::SP_M);
        ui.label(RichText::new("Open Remote Sessions to configure a source; credentials remain in the secret store.").color(Style::TEXT));
    });
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn state_card(ui: &mut egui::Ui, title: &str, detail: &str, tone: egui::Color32) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.colored_label(tone, "●");
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(Style::TEXT_STRONG).strong());
                ui.label(RichText::new(detail).small().color(Style::TEXT_DIM));
            });
        });
    });
}

fn skeleton_shelf(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for _ in 0..4 {
            ui.add(
                egui::Label::new(RichText::new("□□□□").size(Style::MUSIC_HERO_ART_NARROW))
                    .sense(Sense::hover()),
            );
            ui.add_space(Style::SP_M);
        }
    });
}

fn track_summary_card(ui: &mut egui::Ui, song: &Song, playing: bool, app: &mut MusicApp) {
    egui::Frame::NONE
        .fill(Style::MUSIC_SURFACE)
        .inner_margin(Style::SP_S as i8)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                app.render_artwork_ref(
                    ui,
                    (!song.cover_art.is_empty()).then_some(song.cover_art.as_str()),
                    &song.id,
                    Style::ICON_XL,
                );
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&song.title)
                            .color(Style::TEXT_STRONG)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            song.artist,
                            if playing { "Playing" } else { "Paused" }
                        ))
                        .color(Style::TEXT_DIM),
                    );
                });
            });
        });
}

fn artwork_tile(ui: &mut egui::Ui, identity: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let palette = [
        Style::MUSIC_GREEN,
        Style::ACCENT_MEDIA,
        Style::ACCENT_MESH,
        Style::ACCENT_SYSTEM,
        Style::ACCENT_WEB,
    ];
    let hash = identity.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(usize::from(byte))
    });
    let color = palette[hash % palette.len()];
    ui.painter()
        .rect_filled(rect, Style::RADIUS_M, color.gamma_multiply(0.86));
    let initials: String = identity
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initials,
        Style::typography_font(mde_egui::TypographyRole::Headline),
        Style::MUSIC_ON_GREEN,
    );
}

fn artwork_ref_hash(value: &str) -> u64 {
    value
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| hash ^ u64::from(byte))
}

impl App for MusicApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain everything the worker has sent since the last frame.
        music_pump(self);

        egui::CentralPanel::default().show(ctx, |ui| music_workspace(ui, self));
    }
}

/// A **designed transient state** — a centred message, with the brand-accent
/// spinner above it while `busy` — for the library / album loading and empty
/// branches, so an in-flight or empty surface reads as a deliberate state rather
/// than a lone dim line pinned to the top-left corner (§7 — an honest "nothing
/// yet", never a mockup). Draws only through the shared `Style`: the spinner
/// takes [`Style::ACCENT`] (the one progress token, CRAFT §7) and the message the
/// dim secondary tone — no raw colour, no literal size.
fn centered_state(ui: &mut egui::Ui, busy: bool, message: &str) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        if busy {
            ui.add(egui::Spinner::new().color(Style::ACCENT).size(Style::SP_L));
            ui.add_space(Style::SP_S);
        }
        ui.label(
            RichText::new(message)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
    });
}

/// Ease a **hover treatment** onto a just-built list row through the shared FAST
/// motion so the row responds to the pointer instead of snapping (CRAFT §4 — hover
/// changes state, so it animates). One hover progress `t` drives two eased layers:
///
/// * a **wash** behind the row content — `band` is the painter slot reserved
///   *before* the row (the repo's reserved-shape idiom, so it renders underneath)
///   — the hovered-surface fill ([`Style::SURFACE_HI`]) faded by the 0→1 progress
///   at the shared card radius ([`Style::RADIUS_M`], matching [`mde_egui::card`]);
/// * a slim **leading accent tab** over the row's left gutter in the surface's own
///   Media group accent ([`Style::ACCENT_MEDIA`] — the same tint the menu bar
///   wears), so the hovered row reads as the live one. Its presence eases in with
///   [`Motion::hover_lift`] over the same progress, sitting in the card's padding
///   gutter so it never crosses the row text.
///
/// `id` keys the per-row animation. Consumes only shared tokens — no raw colour,
/// size, or literal duration — and, because `t` comes from [`Motion::animate`],
/// reduce-motion collapses both layers to their endpoint (a snap, no glide;
/// a11y-07).
fn hover_indicator(
    ui: &egui::Ui,
    band: egui::layers::ShapeIdx,
    id: impl std::hash::Hash,
    response: &Response,
) {
    let t = Motion::animate(ui.ctx(), id, response.hovered(), Motion::FAST);
    if t <= 0.0 {
        return;
    }
    let row = response.rect;
    ui.painter().set(
        band,
        egui::Shape::rect_filled(row, Style::RADIUS_M, Style::SURFACE_HI.gamma_multiply(t)),
    );
    // The accent tab paints after the card (on top), eased in over the same hover
    // progress; the card's SP_M padding keeps it clear of the row content.
    ui.painter().rect_filled(
        hover_tab_rect(row),
        Style::RADIUS_S,
        Style::ACCENT_MEDIA.gamma_multiply(Motion::hover_lift(t)),
    );
}

/// The slim leading **accent-tab** rect for a hovered row: [`Style::SP_XS`] wide,
/// pinned to the row's left edge and inset top and bottom by [`Style::SP_S`] so it
/// reads as a tab rather than a full-height rule. Pure, so the geometry stays on
/// the 8px grid and is unit-tested without a GPU.
fn hover_tab_rect(row: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(row.left(), row.top() + Style::SP_S),
        egui::pos2(row.left() + Style::SP_XS, row.bottom() - Style::SP_S),
    )
}

/// One clickable album row: title over the `artist · tracks · year` subtitle, in
/// a bordered surface that turns the cursor to a pointing hand on hover. Rendered
/// through the shared `Style` visuals (no raw colours).
fn album_row(ui: &mut egui::Ui, album: &Album) -> Response {
    // Reserve the wash slot so it paints BEHIND the row content (the repo idiom).
    let band = ui.painter().add(egui::Shape::Noop);
    let group = mde_egui::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.vertical(|ui| {
            ui.label(
                RichText::new(&album.name)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            let subtitle = album_subtitle(album);
            if !subtitle.is_empty() {
                mde_egui::muted_note(ui, subtitle);
            }
        });
    });
    let response = group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    hover_indicator(ui, band, ("album-row", album.id.as_str()), &response);
    response
}

/// One clickable track row: track number, title, and right-aligned duration.
/// Clicking the row plays the track. `index` provides a 1-based fallback number
/// when the server didn't tag the track.
fn track_row(ui: &mut egui::Ui, index: usize, song: &Song) -> Response {
    let number = song
        .track
        .map_or_else(|| (index + 1).to_string(), |t| t.to_string());
    // Reserve the wash slot so it paints BEHIND the row content (the repo idiom).
    let band = ui.painter().add(egui::Shape::Noop);
    let group = mde_egui::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{number:>2}"))
                    .monospace()
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(&song.title)
                    .size(Style::BODY)
                    .color(Style::TEXT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format_duration(u64::from(song.duration)))
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            });
        });
    });
    let response = group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    hover_indicator(ui, band, ("track-row", song.id.as_str()), &response);
    response
}

#[cfg(test)]
mod tests {
    use super::{
        bookmark_play_request, hover_tab_rect, music_header, music_panel, music_pump,
        render_now_playing_rail, LibraryFilter, MusicApp, MusicRoute, MUSIC_DETAIL_REQUEST_GRACE,
    };
    use crate::menubar::MenuAction;
    use crate::model::{Fetch, MusicState, Update};
    use crate::workspace_reader::WorkspaceReader;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_musicd::airsonic::{Album, Song};
    use mde_musicd::domain::{
        BookmarkItem, CatalogItem, ContentKind, ContentRef, HomeShelf, LibraryCollection,
        MusicActionRequestV1, MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot,
        PlaybackTarget, QueueEntry, ServerCapabilities, SourceVariant,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn album(id: &str) -> Album {
        Album {
            id: id.to_string(),
            name: format!("Album {id}"),
            artist: "Artist".to_string(),
            artist_id: String::new(),
            song_count: 2,
            cover_art: String::new(),
            year: Some(2021),
        }
    }

    fn song(id: &str) -> Song {
        Song {
            id: id.to_string(),
            title: format!("Track {id}"),
            album: "Album".to_string(),
            artist: "Artist".to_string(),
            duration: 180,
            track: None,
            suffix: "flac".to_string(),
            cover_art: String::new(),
        }
    }

    fn empty_workspace_snapshot(revision: u64) -> MusicWorkspaceSnapshotV1 {
        MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 0,
                seekable: false,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        }
    }

    /// Build a `MusicApp` around a given `state` with no worker and no credentials
    /// — the embedded case a shell would drive, minus the daemon. `music_panel`
    /// never touches the update channel, so a disconnected receiver is fine.
    fn app_with(state: MusicState, setup_error: Option<String>) -> MusicApp {
        let (_tx, rx) = mpsc::sync_channel::<Update>(crate::worker::UPDATE_QUEUE_CAPACITY);
        MusicApp {
            state,
            commands: None,
            updates: rx,
            server: "airsonic.mesh:4040".to_string(),
            setup_error,
            ctx: egui::Context::default(),
            update_tx: _tx,
            next_creds_check: std::time::Instant::now(),
            worker_enabled: true,
            route: MusicRoute::Home,
            library_filter: LibraryFilter::All,
            open_catalog_detail: None,
            pending_detail_request: None,
            workspace_reader: WorkspaceReader::from_root(None),
            next_workspace_poll: Instant::now(),
            workspace_action_publisher: None,
            workspace_browse_publisher: None,
            search_query: String::new(),
            search_generation: 0,
            search_deadline: None,
            browse_collections: BTreeMap::new(),
            artwork_textures: BTreeMap::new(),
            artwork_requests: BTreeSet::new(),
            artwork_failures: BTreeSet::new(),
            library_collapsed: false,
            now_playing_open: true,
            library_width: Style::MUSIC_LIBRARY_RAIL,
            now_playing_width: Style::MUSIC_NOW_PLAYING_RAIL,
        }
    }

    fn daemon_app_with(state: MusicState, setup_error: Option<String>) -> MusicApp {
        let mut app = app_with(state, setup_error);
        app.worker_enabled = false;
        app
    }

    /// Drive one headless egui frame that shows `music_panel`, then tessellate the
    /// result on the CPU so any paint-path fault (bad shape/text/geometry) surfaces
    /// as a test failure. This is the same `Context::run` → `tessellate` path the
    /// DRM runner drives, minus the GPU — no window, no wgpu, no sound device — so
    /// the embeddable panel is proven runtime-reachable in `cargo test`. Returns
    /// the frame's shapes so presentation tests can assert off what painted.
    fn render_shapes(app: &mut MusicApp) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| music_panel(ui, app));
        });
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
        out.shapes
    }

    fn render_full_shapes(app: &mut MusicApp) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("music-header-test").show(ctx, |ui| music_header(ui, app));
            egui::CentralPanel::default().show(ctx, |ui| music_panel(ui, app));
        });
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "full music frame produced no draw primitives"
        );
        out.shapes
    }

    fn render_now_playing_shapes(app: &mut MusicApp) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1440.0, 900.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render_now_playing_rail(ui, app));
        });
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "now-playing rail produced no draw primitives"
        );
        out.shapes
    }

    fn render(app: &mut MusicApp) {
        let _ = render_shapes(app);
    }

    /// Every painted text run (string + font size) from a frame's shapes.
    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, f32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let size = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map_or(0.0, |s| s.format.font_id.size);
                    out.push((text.galley.text().to_owned(), size));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn setup_needed_renders_without_credentials() {
        // No creds ⇒ the honest "connect a server" state (§7), the path an
        // unconfigured embed opens to — rendered end-to-end, no worker spawned.
        let mut app = app_with(
            MusicState::new(),
            Some("no music server configured (run `mde-musicd --first-run`)".to_string()),
        );
        render(&mut app);
    }

    #[test]
    fn library_listing_and_states_render() {
        // A populated library exercises album_row for every row.
        let mut ready = MusicState::new();
        ready.albums = Fetch::Ready(vec![album("1"), album("2")]);
        render(&mut app_with(ready, None));

        // The loading / failed / empty branches each paint their honest line.
        let mut loading = MusicState::new();
        loading.albums = Fetch::Loading;
        render(&mut app_with(loading, None));

        let mut failed = MusicState::new();
        failed.albums = Fetch::Failed("server down".to_string());
        render(&mut app_with(failed, None));

        let mut empty = MusicState::new();
        empty.albums = Fetch::Ready(Vec::new());
        render(&mut app_with(empty, None));
    }

    #[test]
    fn empty_daemon_snapshot_does_not_reveal_legacy_home_catalog() {
        let mut state = MusicState::new();
        state.albums = Fetch::Ready(vec![album("legacy")]);
        state.starred = Fetch::Ready(vec![album("legacy-starred")]);
        state.workspace = Some(empty_workspace_snapshot(21));

        let texts = painted_text(&render_full_shapes(&mut app_with(state, None)));
        assert!(texts
            .iter()
            .any(|(text, _)| text == "Music catalog unavailable"));
        assert!(!texts.iter().any(|(text, _)| text == "Album legacy"));
        assert!(!texts.iter().any(|(text, _)| text == "Album legacy-starred"));
    }

    #[test]
    fn embedded_surface_waits_for_daemon_instead_of_revealing_legacy_state() {
        let mut state = MusicState::new();
        state.albums = Fetch::Ready(vec![album("legacy")]);
        state.starred = Fetch::Ready(vec![album("legacy-starred")]);
        let mut app = daemon_app_with(state, None);

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts
            .iter()
            .any(|(text, _)| text == "Waiting for Music daemon state"));
        assert!(!texts.iter().any(|(text, _)| text == "Album legacy"));
        assert!(!texts.iter().any(|(text, _)| text == "Album legacy-starred"));

        app.update_tx
            .try_send(Update::Started(song("stale-worker")))
            .expect("hostile stale worker update should fit the test queue");
        music_pump(&mut app);
        assert!(app.state.now_playing.is_none());
    }

    #[test]
    fn daemon_snapshot_without_browse_writer_does_not_fall_back_to_worker_search() {
        let mut app = app_with(MusicState::new(), None);
        app.state.workspace = Some(empty_workspace_snapshot(22));
        app.search_query = "legacy query".to_owned();

        app.issue_search();

        assert!(matches!(app.state.search, Fetch::Loading));
        assert_eq!(
            app.state.error.as_deref(),
            Some(
                "Music search is unavailable until the authenticated daemon browse path is connected."
            )
        );
    }

    #[test]
    fn daemon_bookmarks_render_as_typed_resume_metadata() {
        let mut state = MusicState::new();
        state.workspace = Some(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 4,
            shelves: Vec::new(),
            bookmarks: vec![BookmarkItem {
                content: ContentRef::new("source-one", "episode-1", ContentKind::Episode)
                    .expect("bookmark identity"),
                title: "Episode one".to_string(),
                creator: "Host".to_string(),
                parent_title: "Podcast".to_string(),
                position_ms: 90_000,
                duration_ms: Some(300_000),
                artwork_ref: None,
            }],
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_string(),
                queue_revision: 0,
                seekable: false,
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
        let texts = painted_text(&render_full_shapes(&mut app_with(state, None)));
        assert!(texts.iter().any(|(text, _)| text == "Resume"));
        assert!(texts.iter().any(|(text, _)| text == "Episode one"));
        assert!(texts
            .iter()
            .any(|(text, _)| text.contains("Host · Podcast")));
        assert!(texts.iter().any(|(text, _)| text == "1:30"));
    }

    #[test]
    fn embedded_connection_status_uses_daemon_source_truth() {
        let mut connected = empty_workspace_snapshot(30);
        connected.any_source_reachable = true;
        let app = daemon_app_with(
            MusicState {
                workspace: Some(connected),
                ..MusicState::new()
            },
            None,
        );
        assert_eq!(app.connection_status().0, "Connected");

        let app = daemon_app_with(
            MusicState {
                workspace: Some(empty_workspace_snapshot(31)),
                ..MusicState::new()
            },
            None,
        );
        assert_eq!(app.connection_status().0, "Source unavailable");
    }

    #[test]
    fn daemon_sources_route_renders_capabilities_and_targets() {
        let mut snapshot = empty_workspace_snapshot(32);
        snapshot.any_source_reachable = true;
        snapshot.sources.push(ServerCapabilities {
            source_id: "airsonic-main".to_owned(),
            api_profile: "OpenSubsonic".to_owned(),
            reachable: true,
            authentication_required: false,
            features: ["search".to_owned(), "stream".to_owned()]
                .into_iter()
                .collect(),
        });
        let mut app = daemon_app_with(
            MusicState {
                workspace: Some(snapshot),
                ..MusicState::new()
            },
            None,
        );
        app.route = MusicRoute::Sources;
        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Sources"));
        assert!(texts.iter().any(|(text, _)| text == "airsonic-main"));
        assert!(texts.iter().any(|(text, _)| text == "OpenSubsonic"));
        assert!(texts
            .iter()
            .any(|(text, _)| text.contains("search · stream")));
    }

    #[test]
    fn daemon_catalog_shelf_renders_and_plays_selected_source_variant() {
        let content = ContentRef::new("source-one", "song-1", ContentKind::Music).unwrap();
        let item = CatalogItem {
            id: "song-1".to_string(),
            kind: ContentKind::Music,
            title: "Daemon song".to_string(),
            creator: "Daemon artist".to_string(),
            parent_title: "Daemon album".to_string(),
            duration_ms: Some(180_000),
            artwork_ref: None,
            starred: false,
            cached: true,
            variants: vec![SourceVariant {
                content: content.clone(),
                cached: true,
                reachable: false,
                operator_priority: 10,
                latency_ms: None,
            }],
        };
        let mut state = MusicState::new();
        state.workspace = Some(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 12,
            shelves: vec![HomeShelf {
                key: "library".to_string(),
                title: "Daemon Library".to_string(),
                items: vec![item.clone()],
            }],
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_string(),
                queue_revision: 0,
                seekable: false,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        });
        let mut app = app_with(state, None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_string())
                .map_err(|error| error.to_string())
        });

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Daemon Library"));
        assert!(texts.iter().any(|(text, _)| text == "Daemon song"));
        assert!(texts.iter().any(|(text, _)| text == "Cached"));

        app.play_catalog_item(&item);
        let request: MusicActionRequestV1 =
            serde_json::from_str(&published_rx.try_recv().expect("typed play request")).unwrap();
        assert_eq!(request.action, "play");
        assert_eq!(request.content.as_ref(), Some(&content));
    }

    #[test]
    fn daemon_library_prefers_typed_collections_over_legacy_album_store() {
        let content = ContentRef::new("source-one", "song-2", ContentKind::Music).unwrap();
        let item = CatalogItem {
            id: "song-2".to_string(),
            kind: ContentKind::Music,
            title: "Daemon library song".to_string(),
            creator: "Daemon artist".to_string(),
            parent_title: "Daemon album".to_string(),
            duration_ms: Some(120_000),
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: content.clone(),
                cached: false,
                reachable: true,
                operator_priority: 1,
                latency_ms: Some(4),
            }],
        };
        let mut state = MusicState::new();
        state.albums = Fetch::Ready(vec![album("legacy")]);
        state.workspace = Some(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 13,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: vec![LibraryCollection {
                key: "songs".to_string(),
                title: "Songs".to_string(),
                kind: ContentKind::Music,
                items: vec![item.clone()],
                mutable: false,
                offset: 0,
                page_size: 0,
                has_more: false,
            }],
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_string(),
                queue_revision: 0,
                seekable: false,
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
        let mut app = app_with(state, None);
        app.route = MusicRoute::Library;
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_string())
                .map_err(|error| error.to_string())
        });

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Songs"));
        assert!(texts.iter().any(|(text, _)| text == "Daemon library song"));
        assert!(!texts.iter().any(|(text, _)| text == "Album legacy"));

        app.play_catalog_item(&item);
        let request: MusicActionRequestV1 =
            serde_json::from_str(&published_rx.try_recv().expect("typed library play request"))
                .unwrap();
        assert_eq!(request.action, "play");
        assert_eq!(request.content.as_ref(), Some(&content));
    }

    #[test]
    fn daemon_album_detail_uses_typed_song_collection_and_typed_play() {
        let album = CatalogItem {
            id: "album-1".to_owned(),
            kind: ContentKind::Album,
            title: "Daemon album".to_owned(),
            creator: "Daemon artist".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: Vec::new(),
        };
        let content = ContentRef::new("source-one", "song-3", ContentKind::Music).unwrap();
        let track = CatalogItem {
            id: "song-3".to_owned(),
            kind: ContentKind::Music,
            title: "Typed album track".to_owned(),
            creator: "Daemon artist".to_owned(),
            parent_title: "Daemon album".to_owned(),
            duration_ms: Some(180_000),
            artwork_ref: None,
            starred: false,
            cached: true,
            variants: vec![SourceVariant {
                content: content.clone(),
                cached: true,
                reachable: false,
                operator_priority: 2,
                latency_ms: None,
            }],
        };
        let mut state = MusicState::new();
        state.workspace = Some(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 14,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: vec![
                LibraryCollection {
                    key: "albums".to_owned(),
                    title: "Albums".to_owned(),
                    kind: ContentKind::Album,
                    items: vec![album.clone()],
                    mutable: false,
                    offset: 0,
                    page_size: 0,
                    has_more: false,
                },
                LibraryCollection {
                    key: "songs".to_owned(),
                    title: "Songs".to_owned(),
                    kind: ContentKind::Music,
                    items: vec![track.clone()],
                    mutable: false,
                    offset: 0,
                    page_size: 0,
                    has_more: false,
                },
            ],
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 0,
                seekable: false,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        });
        let mut app = app_with(state, None);
        app.open_daemon_album(&album);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Daemon album"));
        assert!(texts.iter().any(|(text, _)| text == "Typed album track"));
        assert!(!texts.iter().any(|(text, _)| text == "Loading tracks…"));

        app.play_catalog_item(&track);
        let request: MusicActionRequestV1 =
            serde_json::from_str(&published_rx.try_recv().expect("typed album play request"))
                .unwrap();
        assert_eq!(request.action, "play");
        assert_eq!(request.content.as_ref(), Some(&content));

        app.download_catalog_item(&track);
        let request: MusicActionRequestV1 = serde_json::from_str(
            &published_rx
                .try_recv()
                .expect("typed album download request"),
        )
        .unwrap();
        assert_eq!(request.action, "download");
        assert_eq!(request.content.as_ref(), Some(&content));
    }

    #[test]
    fn embedded_search_uses_daemon_browse_publisher_instead_of_worker() {
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_browse_publisher(move |verb, body| {
            published_tx
                .send((verb.to_string(), body.to_string()))
                .map_err(|error| error.to_string())
        });
        app.search_query = "blue hour".to_string();

        app.issue_search();

        let (verb, body) = published_rx
            .try_recv()
            .expect("daemon browse request should be published");
        assert_eq!(verb, "search");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["query"],
            "blue hour"
        );
        assert!(matches!(app.state.search, Fetch::Loading));
    }

    #[test]
    fn library_hub_filters_publish_the_provider_browse_seam() {
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(4);
        app.set_workspace_browse_publisher(move |verb, body| {
            published_tx
                .send((verb.to_owned(), body.to_owned()))
                .map_err(|error| error.to_string())
        });

        for (filter, expected) in [
            (LibraryFilter::Artists, "list-artists"),
            (LibraryFilter::Podcasts, "list-podcasts"),
            (LibraryFilter::Radio, "list-radio"),
            (LibraryFilter::Albums, "list-albums"),
        ] {
            app.select_library_filter(filter);
            let (verb, body) = published_rx.try_recv().expect("browse request");
            assert_eq!(verb, expected);
            let body = serde_json::from_str::<serde_json::Value>(&body).unwrap();
            assert_eq!(body["offset"], 0);
            assert_eq!(body["size"], 100);
            assert_eq!(app.route, MusicRoute::Library);
            assert_eq!(app.library_filter, filter);
        }
    }

    #[test]
    fn browse_pages_replace_bounded_windows_and_advance_from_page_metadata() {
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(2);
        app.set_workspace_browse_publisher(move |verb, body| {
            published_tx
                .send((verb.to_owned(), body.to_owned()))
                .map_err(|error| error.to_string())
        });

        let item = |index: usize| CatalogItem {
            id: format!("artist-{index}"),
            kind: ContentKind::Artist,
            title: format!("Artist {index}"),
            creator: String::new(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: Vec::new(),
        };

        app.retain_browse_page("artists", 0, 100, true, (0..120).map(item).collect());
        let state = app
            .browse_collections
            .get("artists")
            .expect("retained artist page");
        assert_eq!(state.items.len(), 100);
        assert_eq!(state.next_offset, 100);
        assert_eq!(state.has_more, Some(true));

        app.request_next_browse_page("artists", ContentKind::Artist);
        let (verb, body) = published_rx.try_recv().expect("next artist page request");
        assert_eq!(verb, "list-artists");
        let body = serde_json::from_str::<serde_json::Value>(&body).unwrap();
        assert_eq!(body["offset"], 100);
        assert_eq!(body["size"], 100);

        app.retain_browse_page("artists", 100, 100, false, (100..220).map(item).collect());
        let state = app
            .browse_collections
            .get("artists")
            .expect("retained artist pages");
        assert_eq!(state.items.len(), 100);
        assert_eq!(state.items[0].id, "artist-100");
        assert_eq!(state.next_offset, 200);
        assert_eq!(state.has_more, Some(false));
    }

    #[test]
    fn daemon_collection_exposes_load_more_after_a_full_fallback_page() {
        let mut snapshot = empty_workspace_snapshot(33);
        snapshot.any_source_reachable = true;
        snapshot.collections.push(LibraryCollection {
            key: "artists".to_owned(),
            title: "Artists".to_owned(),
            kind: ContentKind::Artist,
            items: (0..100)
                .map(|index| CatalogItem {
                    id: format!("artist-{index}"),
                    kind: ContentKind::Artist,
                    title: format!("Artist {index}"),
                    creator: String::new(),
                    parent_title: String::new(),
                    duration_ms: None,
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: Vec::new(),
                })
                .collect(),
            mutable: false,
            offset: 0,
            page_size: 100,
            has_more: true,
        });
        let mut app = daemon_app_with(
            MusicState {
                workspace: Some(snapshot),
                ..MusicState::new()
            },
            None,
        );
        app.route = MusicRoute::Library;
        app.library_filter = LibraryFilter::Artists;

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Load more"));
    }

    #[test]
    fn daemon_non_album_rows_open_typed_detail_seams() {
        let item = |kind: ContentKind, title: &str, remote_id: &str| CatalogItem {
            id: format!("{kind:?}-{title}"),
            kind,
            title: title.to_owned(),
            creator: String::new(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new("source-one", remote_id, kind).unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(2);
        app.set_workspace_browse_publisher(move |verb, body| {
            published_tx
                .send((verb.to_owned(), body.to_owned()))
                .map_err(|error| error.to_string())
        });

        app.open_catalog_detail(&item(ContentKind::Artist, "Artist", "artist-1"));
        assert_eq!(app.route, MusicRoute::Artist);
        let (verb, body) = published_rx.try_recv().expect("artist detail request");
        assert_eq!(verb, "albums-by-artist");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"],
            "artist-1"
        );

        app.open_catalog_detail(&item(ContentKind::Podcast, "Podcast", "feed-1"));
        assert_eq!(app.route, MusicRoute::Podcast);
        let (verb, body) = published_rx.try_recv().expect("podcast detail request");
        assert_eq!(verb, "podcast-episodes");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"],
            "feed-1"
        );

        app.open_catalog_detail(&item(
            ContentKind::Radio,
            "Radio",
            "https://radio.test/live",
        ));
        assert_eq!(app.route, MusicRoute::Radio);
        assert!(published_rx.try_recv().is_err());
    }

    #[test]
    fn daemon_artist_detail_waits_for_matching_retained_response() {
        let artist = CatalogItem {
            id: "artist-38-special".to_owned(),
            kind: ContentKind::Artist,
            title: "38 Special".to_owned(),
            creator: String::new(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new("source-one", "1", ContentKind::Artist).unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut snapshot = empty_workspace_snapshot(40);
        snapshot.any_source_reachable = true;
        let mut app = daemon_app_with(
            MusicState {
                workspace: Some(snapshot),
                ..MusicState::new()
            },
            None,
        );
        app.set_workspace_browse_publisher(|_, _| Ok(()));

        app.open_catalog_detail(&artist);

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Loading albums…"));
        assert!(!texts.iter().any(|(text, _)| text == "No detail rows"));

        app.state
            .workspace
            .as_mut()
            .unwrap()
            .collections
            .push(LibraryCollection {
                key: "albums".to_owned(),
                title: "Albums".to_owned(),
                kind: ContentKind::Album,
                items: vec![CatalogItem {
                    id: "album-wild-eyed-southern-boys".to_owned(),
                    kind: ContentKind::Album,
                    title: "Wild-Eyed Southern Boys".to_owned(),
                    creator: "38 Special".to_owned(),
                    parent_title: String::new(),
                    duration_ms: None,
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: Vec::new(),
                }],
                mutable: false,
                offset: 0,
                page_size: 1,
                has_more: false,
            });
        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts
            .iter()
            .any(|(text, _)| text == "Wild-Eyed Southern Boys"));
        assert!(app.pending_detail_request.is_none());
    }

    #[test]
    fn daemon_podcast_detail_renders_in_flight_state() {
        let podcast = CatalogItem {
            id: "podcast-wait-wait".to_owned(),
            kind: ContentKind::Podcast,
            title: "Wait Wait... Don't Tell Me!".to_owned(),
            creator: String::new(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new("source-one", "0", ContentKind::Podcast).unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut snapshot = empty_workspace_snapshot(41);
        snapshot.any_source_reachable = true;
        let mut app = daemon_app_with(
            MusicState {
                workspace: Some(snapshot),
                ..MusicState::new()
            },
            None,
        );
        app.set_workspace_browse_publisher(|_, _| Ok(()));

        app.open_catalog_detail(&podcast);

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Loading episodes…"));
        assert!(!texts.iter().any(|(text, _)| text == "No detail rows"));
    }

    #[test]
    fn daemon_album_detail_does_not_report_stale_snapshot_as_unavailable() {
        let album = CatalogItem {
            id: "album-black-ice".to_owned(),
            kind: ContentKind::Album,
            title: "Black Ice".to_owned(),
            creator: "AC/DC".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new("source-one", "11", ContentKind::Album).unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut snapshot = empty_workspace_snapshot(42);
        snapshot.any_source_reachable = true;
        let mut app = daemon_app_with(
            MusicState {
                workspace: Some(snapshot),
                ..MusicState::new()
            },
            None,
        );
        app.set_workspace_browse_publisher(|_, _| Ok(()));

        app.open_daemon_album(&album);

        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Loading tracks…"));
        assert!(!texts.iter().any(|(text, _)| text == "Album unavailable"));

        app.pending_detail_request.as_mut().unwrap().started = Instant::now()
            .checked_sub(MUSIC_DETAIL_REQUEST_GRACE + Duration::from_millis(1))
            .unwrap();
        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Album unavailable"));
        assert!(app.pending_detail_request.is_none());
    }

    #[test]
    fn radio_catalog_row_opens_detail_and_explicit_play_publishes_typed_stream() {
        let station = CatalogItem {
            id: "radio-cspan".to_owned(),
            kind: ContentKind::Radio,
            title: "C-SPAN Radio".to_owned(),
            creator: "Internet radio".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![
                SourceVariant {
                    content: ContentRef::new(
                        "airsonic:http://radio.test",
                        "provider-station-id-without-a-stream",
                        ContentKind::Radio,
                    )
                    .unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 10,
                    latency_ms: None,
                },
                SourceVariant {
                    content: ContentRef::new(
                        "airsonic:http://radio.test",
                        "https://stream.test/cspan-live",
                        ContentKind::Radio,
                    )
                    .unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 0,
                    latency_ms: None,
                },
            ],
        };
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        app.activate_catalog_item(&station);

        assert_eq!(app.route, MusicRoute::Radio);
        assert_eq!(
            app.open_catalog_detail
                .as_ref()
                .map(|item| item.id.as_str()),
            Some("radio-cspan")
        );
        assert!(
            published_rx.try_recv().is_err(),
            "catalog-row activation must not mutate playback"
        );
        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Play station"));
        assert!(texts
            .iter()
            .any(|(text, _)| text == "Direct stream target admitted"));

        app.play_radio_station(&station);

        let request: MusicActionRequestV1 =
            serde_json::from_str(&published_rx.try_recv().expect("radio play request")).unwrap();
        assert_eq!(request.action, "play");
        assert_eq!(request.content.as_ref().unwrap().kind, ContentKind::Radio);
        assert_eq!(
            request.content.as_ref().unwrap().remote_id,
            "https://stream.test/cspan-live"
        );
    }

    #[test]
    fn withdrawn_radio_detail_cannot_publish_its_stale_stream_identity() {
        let station = CatalogItem {
            id: "radio-stale".to_owned(),
            kind: ContentKind::Radio,
            title: "Withdrawn station".to_owned(),
            creator: "Internet radio".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new(
                    "airsonic:http://radio.test",
                    "https://stream.test/withdrawn",
                    ContentKind::Radio,
                )
                .unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut retained = empty_workspace_snapshot(40);
        retained.collections.push(LibraryCollection {
            key: "radio".to_owned(),
            title: "Radio".to_owned(),
            kind: ContentKind::Radio,
            items: vec![station.clone()],
            mutable: false,
            offset: 0,
            page_size: 1,
            has_more: false,
        });
        let mut state = MusicState::new();
        state.workspace = Some(retained);
        let mut app = app_with(state, None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });
        app.activate_catalog_item(&station);

        app.state.workspace = Some(empty_workspace_snapshot(41));
        app.play_radio_station(&station);

        assert!(published_rx.try_recv().is_err());
        assert_eq!(
            app.state.error.as_deref(),
            Some(
                "Withdrawn station changed or was withdrawn; reopen the station from the latest Music catalog"
            )
        );
    }

    #[test]
    fn empty_and_malformed_radio_streams_render_unavailable_and_publish_nothing() {
        let station = CatalogItem {
            id: "radio-unavailable".to_owned(),
            kind: ContentKind::Radio,
            title: "Unavailable station".to_owned(),
            creator: "Internet radio".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![
                SourceVariant {
                    content: ContentRef {
                        source_id: "airsonic:http://radio.test".to_owned(),
                        remote_id: String::new(),
                        kind: ContentKind::Radio,
                    },
                    cached: false,
                    reachable: true,
                    operator_priority: 10,
                    latency_ms: None,
                },
                SourceVariant {
                    content: ContentRef::new(
                        "airsonic:http://radio.test",
                        "https:///missing-host",
                        ContentKind::Radio,
                    )
                    .unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 5,
                    latency_ms: None,
                },
                SourceVariant {
                    content: ContentRef::new(
                        "airsonic:http://radio.test",
                        "ftp://stream.test/not-http",
                        ContentKind::Radio,
                    )
                    .unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 0,
                    latency_ms: None,
                },
            ],
        };
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        app.activate_catalog_item(&station);

        assert_eq!(app.route, MusicRoute::Radio);
        assert!(published_rx.try_recv().is_err());
        let texts = painted_text(&render_full_shapes(&mut app));
        assert!(texts.iter().any(|(text, _)| text == "Station unavailable"));
        assert!(!texts.iter().any(|(text, _)| text == "Play station"));

        app.play_radio_station(&station);

        assert!(published_rx.try_recv().is_err());
        assert_eq!(
            app.state.error.as_deref(),
            Some("Unavailable station is unavailable: no admitted direct HTTP stream target")
        );
    }

    #[test]
    fn daemon_album_open_requests_provider_track_detail() {
        let album = CatalogItem {
            id: "album-1".to_owned(),
            kind: ContentKind::Album,
            title: "Mesh album".to_owned(),
            creator: "Mesh artist".to_owned(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content: ContentRef::new("source-one", "album-remote-1", ContentKind::Album)
                    .unwrap(),
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_browse_publisher(move |verb, body| {
            published_tx
                .send((verb.to_owned(), body.to_owned()))
                .map_err(|error| error.to_string())
        });

        app.open_daemon_album(&album);

        let (verb, body) = published_rx.try_recv().expect("album detail request");
        assert_eq!(verb, "get-album");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"],
            "album-remote-1"
        );
        assert_eq!(app.route, MusicRoute::Album);
    }

    #[test]
    fn embedded_constructor_does_not_start_standalone_worker() {
        let app = MusicApp::new_embedded_with_ctx(&egui::Context::default());
        assert!(app.commands.is_none());
        assert!(!app.worker_enabled);
    }

    #[test]
    fn mesh_target_handoff_emits_typed_transfer_request() {
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });
        let target = PlaybackTarget {
            id: "peer:seat-15".to_owned(),
            name: "Seat 15".to_owned(),
            kind: "mesh_seat".to_owned(),
            available: true,
            unavailable_reason: None,
        };

        app.transfer_playback_to_target(&target);

        let request: MusicActionRequestV1 =
            serde_json::from_str(&published_rx.try_recv().expect("typed transfer request"))
                .unwrap();
        assert_eq!(request.action, "transfer");
        assert_eq!(request.target_peer.as_deref(), Some("peer:seat-15"));
    }

    #[test]
    fn restarted_daemon_target_cannot_authorize_retained_handoff_identity() {
        let retained_target = PlaybackTarget {
            id: "peer:seat-15".to_owned(),
            name: "Seat 15".to_owned(),
            kind: "mesh_seat".to_owned(),
            available: true,
            unavailable_reason: None,
        };
        let mut initial = empty_workspace_snapshot(7);
        initial.targets = vec![retained_target.clone()];
        let mut state = MusicState::new();
        state.workspace = Some(initial);
        let mut app = app_with(state, None);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        let mut replacement = empty_workspace_snapshot(8);
        replacement.targets = vec![PlaybackTarget {
            id: retained_target.id.clone(),
            name: "Recycled seat identity".to_owned(),
            kind: "mesh_seat".to_owned(),
            available: false,
            unavailable_reason: Some("replacement daemon has not proved ownership".to_owned()),
        }];
        app.state.workspace = Some(replacement);
        app.transfer_playback_to_target(&retained_target);

        assert!(published_rx.try_recv().is_err());
        assert_eq!(
            app.state.error.as_deref(),
            Some(
                "Seat 15 changed or was withdrawn; choose a target from the latest Music projection"
            )
        );
    }

    #[test]
    fn daemon_queue_projection_renders_typed_entries_and_current_marker() {
        let content = ContentRef::new("source-one", "song-1", ContentKind::Music).unwrap();
        let mut state = MusicState::new();
        state.workspace = Some(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 9,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: Some(content.clone()),
                playing: true,
                position_ms: 1_000,
                duration_ms: Some(180_000),
                volume_milli: 900,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 3,
                seekable: true,
            },
            queue: vec![QueueEntry {
                id: "entry-1".to_owned(),
                content,
                title: "Queued song".to_owned(),
            }],
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: true,
        });
        state.now_playing = Some(song("song-1"));

        let texts = painted_text(&render_now_playing_shapes(&mut app_with(state, None)));
        assert!(texts.iter().any(|(text, _)| text == "Queue"));
        assert!(texts.iter().any(|(text, _)| text.contains("Queued song")));
        assert!(texts.iter().any(|(text, _)| text.contains("▶ 1.")));
        assert!(texts.iter().any(|(text, _)| text.contains("source-one")));
    }

    #[test]
    fn resume_bookmark_request_preserves_source_kind_and_position() {
        let bookmark = BookmarkItem {
            content: ContentRef::new("source-one", "episode-1", ContentKind::Episode).unwrap(),
            title: "Episode one".to_owned(),
            creator: "Host".to_owned(),
            parent_title: "Podcast".to_owned(),
            position_ms: 42_500,
            duration_ms: Some(90_000),
            artwork_ref: None,
        };
        let request = bookmark_play_request(&bookmark).unwrap();
        let body = serde_json::to_string(&request).unwrap();
        assert_eq!(request.action, "play");
        assert_eq!(request.position_ms, Some(42_500));
        assert_eq!(request.content.unwrap().kind, ContentKind::Episode);
        assert!(!body.contains("armed_token"));
    }

    #[test]
    fn shell_transport_publishes_typed_request_and_standalone_fails_closed() {
        let mut app = app_with(MusicState::new(), None);
        let (published_tx, published_rx) = mpsc::sync_channel::<String>(1);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        assert!(app.try_publish_transport_action("pause", None, None));
        let body = published_rx.try_recv().expect("typed transport request");
        let request: MusicActionRequestV1 = serde_json::from_str(&body).unwrap();
        assert_eq!(request.action, "pause");
        assert!(request.content.is_none());
        assert!(request.armed_token.is_none());

        let mut standalone = app_with(MusicState::new(), None);
        standalone.worker_enabled = false;
        assert!(standalone.try_publish_transport_action("pause", None, None));
        assert_eq!(
            standalone.state.error.as_deref(),
            Some("Music transport is unavailable until the authenticated daemon action path is connected.")
        );
    }

    #[test]
    fn queue_controls_publish_exact_rendered_generation_and_policy() {
        let mut state = MusicState::new();
        let mut snapshot = empty_workspace_snapshot(23);
        let content = ContentRef {
            source_id: "airsonic:forge".to_owned(),
            remote_id: "track-7".to_owned(),
            kind: ContentKind::Music,
        };
        snapshot.playback.current = Some(content.clone());
        snapshot.playback.queue_revision = 91;
        snapshot.queue.push(QueueEntry {
            id: "queue-7".to_owned(),
            content,
            title: "Rendered track".to_owned(),
        });
        state.workspace = Some(snapshot);
        let mut app = app_with(state, None);
        let (published_tx, published_rx) = mpsc::sync_channel::<String>(3);
        app.set_workspace_action_publisher(move |body| {
            published_tx
                .send(body.to_owned())
                .map_err(|error| error.to_string())
        });

        app.publish_queue_playback_action("next", None, None);
        app.publish_queue_playback_action("shuffle", Some(true), None);
        app.publish_queue_playback_action("repeat", None, Some("track"));

        let requests = (0..3)
            .map(|_| {
                serde_json::from_str::<MusicActionRequestV1>(
                    &published_rx.try_recv().expect("queue action"),
                )
                .expect("typed queue request")
            })
            .collect::<Vec<_>>();
        assert_eq!(requests[0].action, "next");
        assert_eq!(requests[1].shuffle, Some(true));
        assert_eq!(requests[2].repeat.as_deref(), Some("track"));
        assert!(requests
            .iter()
            .all(|request| request.expected_queue_revision == Some(91)));
    }

    #[test]
    fn standalone_constructor_refuses_hostile_worker_playback_state() {
        let mut app = MusicApp::new_with_ctx(&egui::Context::default());
        assert!(
            app.commands.is_none(),
            "standalone must not start a provider worker"
        );
        assert!(!app.worker_enabled, "standalone must use daemon authority");

        app.update_tx
            .try_send(Update::Started(song("hostile-worker-track")))
            .expect("hostile update should fit the bounded stale queue");
        app.update_tx
            .try_send(Update::Playing(true))
            .expect("hostile update should fit the bounded stale queue");
        music_pump(&mut app);

        assert!(
            app.state.now_playing.is_none(),
            "GUI-owned worker state must not become now-playing authority"
        );
        assert!(
            !app.state.playing,
            "GUI-owned worker state must remain inert"
        );
    }

    #[test]
    fn daemon_transport_without_writer_does_not_queue_a_local_worker_command() {
        let mut state = MusicState::new();
        state.workspace = Some(empty_workspace_snapshot(23));
        let mut app = app_with(state, None);
        let (commands, received) = mpsc::sync_channel(1);
        app.commands = Some(commands);

        assert!(app.try_publish_transport_action("pause", None, None));
        assert!(received.try_recv().is_err());
        assert_eq!(
            app.state.error.as_deref(),
            Some("Music transport is unavailable until the authenticated daemon action path is connected.")
        );
    }

    #[test]
    fn open_album_with_tracks_and_error_banner_render() {
        // Transient engine error + a now-playing track + an open album with tracks
        // exercises the error banner and track_row alongside the album header.
        let mut state = MusicState::new();
        state.error = Some("audio output unavailable".to_string());
        state.now_playing = Some(song("42"));
        state.playing = true;
        state.open(album("7"));
        state.open_album.as_mut().expect("an album is open").tracks =
            Fetch::Ready(vec![song("a"), song("b")]);
        render(&mut app_with(state, None));
    }

    /// The shared Music MenuBar owns the host title. Album identity and its
    /// domain return affordance remain below it without a second AppFrame.
    #[test]
    fn view_headers_avoid_a_second_app_frame() {
        // The listing: the shared host bar owns the Music title.
        let mut ready = MusicState::new();
        ready.albums = Fetch::Ready(vec![album("1")]);
        let texts = painted_text(&render_full_shapes(&mut app_with(ready, None)));
        assert!(
            texts.iter().any(|(t, _)| t == "MUSIC"),
            "the workspace title must render on the shared bar: {texts:?}"
        );

        // The open album: its title is domain content below the shared bar.
        let mut state = MusicState::new();
        state.open(album("7"));
        state.open_album.as_mut().expect("an album is open").tracks = Fetch::Ready(vec![song("a")]);
        let texts = painted_text(&render_full_shapes(&mut app_with(state, None)));
        assert!(
            texts
                .iter()
                .any(|(t, s)| t == "Album 7" && (*s - Style::TYPE_TITLE3).abs() < f32::EPSILON),
            "the album title must render below the shared bar: {texts:?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("Library")),
            "the domain back affordance must remain visible: {texts:?}"
        );
    }

    #[test]
    fn menu_back_to_library_closes_the_open_album() {
        // The View → Back to Library menu action drives the same `close` seam the
        // album view's button does — a real navigation seam, not a no-op.
        let mut state = MusicState::new();
        state.open(album("7"));
        let mut app = app_with(state, None);
        assert!(app.state.open_album.is_some());
        app.run_menu_action(MenuAction::BackToLibrary);
        assert!(
            app.state.open_album.is_none(),
            "Back to Library returned to the listing"
        );
    }

    #[test]
    fn menu_context_snapshots_transport_and_connection() {
        // A worker-less fixture (no creds) with a track playing + an album open:
        // the context mirrors the live state the bar gates + renders from.
        let mut state = MusicState::new();
        state.now_playing = Some(song("42"));
        state.playing = true;
        state.position_ms = 5_000;
        state.open(album("7"));
        let app = app_with(state, None);
        let cx = app.menu_context();
        assert!(!cx.connected, "app_with spawns no worker");
        assert!(cx.has_track && cx.playing);
        assert!(cx.album_open);
        let np = cx.now_playing.expect("a track is playing");
        assert_eq!(np.title, "Track 42");
        // 5000ms → 5s, clamped to the 180s tagged length.
        assert_eq!(np.elapsed_secs, 5);
        assert_eq!(np.duration_secs, 180);
    }

    #[test]
    fn album_rows_adopt_the_shared_card_primitive() {
        // The library/album rows are the shared `mde_egui::card()` surface, so their
        // depth is the foundation's Raised elevation verbatim — no per-surface shadow
        // helper is minted here (§4). A translucent umbra keeps it a soft shadow,
        // never an opaque fill (design lock #2).
        use mde_egui::style::Elevation;
        let card = mde_egui::card();
        assert_eq!(
            card.shadow,
            Elevation::Raised.egui_shadow(),
            "the row card casts the shared Raised soft shadow"
        );
        assert_eq!(
            card.fill,
            Style::SURFACE,
            "the row card fills the base surface"
        );
        let alpha = card.shadow.color.a();
        assert!(
            alpha > 0 && alpha < 255,
            "a Raised card casts a translucent soft shadow (lock #2), got alpha {alpha}"
        );
    }

    #[test]
    fn hover_accent_tab_sits_on_the_left_edge_on_the_grid() {
        // The hovered-row leading accent tab is a pure geometry on the 8px grid:
        // pinned to the row's left edge, one half-step wide, and inset top and
        // bottom by the base unit so it reads as a tab — every extent a Style
        // token, none a raw literal (§4). A tall row keeps the insets well-formed.
        let row = Rect::from_min_max(pos2(10.0, 20.0), pos2(210.0, 80.0));
        let tab = hover_tab_rect(row);
        assert_eq!(tab.left(), row.left(), "the tab hugs the row's left edge");
        assert_eq!(tab.width(), Style::SP_XS, "one 8px-grid half-step wide");
        assert_eq!(
            tab.top(),
            row.top() + Style::SP_S,
            "inset from the top by the base grid unit"
        );
        assert_eq!(
            tab.bottom(),
            row.bottom() - Style::SP_S,
            "inset from the bottom by the base grid unit"
        );
        assert!(
            row.contains_rect(tab),
            "the tab stays within the row it marks"
        );
    }
}
