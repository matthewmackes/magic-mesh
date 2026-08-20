//! Versioned domain contracts shared by the Music daemon and its egui client.
//!
//! These values deliberately contain catalog facts and opaque references only.
//! Endpoint URLs and credentials stay behind the admitted resource adapter. The
//! pure helpers at the end of this module keep aggregation, deduplication,
//! variant choice, shelf construction, and stale-result rejection deterministic
//! and easy to exercise without a server or an audio device.

use serde::{Deserialize, Serialize};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

/// Current retained workspace/action contract version.
pub const MUSIC_CONTRACT_VERSION: u16 = 1;
/// Maximum number of source variants retained beneath one merged item.
pub const MAX_SOURCE_VARIANTS: usize = 16;
/// Maximum number of items in one progressive search page.
pub const MAX_SEARCH_ITEMS: usize = 96;
/// Maximum number of Home shelves retained in one snapshot.
pub const MAX_SHELVES: usize = 32;
/// Maximum queue entries retained in one workspace snapshot.
pub const MAX_QUEUE_ITEMS: usize = 512;
/// Maximum bytes in one queue-stable entry identity.
pub const MAX_QUEUE_ENTRY_ID_BYTES: usize = 256;
/// Maximum library items retained in one collection.
pub const MAX_COLLECTION_ITEMS: usize = 512;
/// Maximum number of items admitted in one provider-backed library page.
pub const MAX_LIBRARY_PAGE_SIZE: usize = 500;
/// Maximum offset accepted for an on-demand library page.
pub const MAX_LIBRARY_OFFSET: usize = 1_000_000;
/// Maximum admitted source records retained in one snapshot.
pub const MAX_SOURCE_RECORDS: usize = 64;
/// Maximum bookmark rows retained in one workspace snapshot.
pub const MAX_BOOKMARKS: usize = 512;
/// Maximum bytes in an action request id.
pub const MAX_REQUEST_ID_BYTES: usize = 128;
/// Maximum playlist name/id or song-id bytes accepted by one typed mutation.
pub const MAX_PLAYLIST_FIELD_BYTES: usize = 256;
/// Maximum number of song ids or removal indexes in one playlist mutation.
pub const MAX_PLAYLIST_ITEMS: usize = 512;
/// Maximum bytes in a target peer identity for a typed playback handoff.
pub const MAX_TARGET_PEER_BYTES: usize = 128;

/// The admitted kinds of audio content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// A playable song.
    Music,
    /// An album or release.
    Album,
    /// An artist or creator.
    Artist,
    /// A native or cross-source playlist.
    Playlist,
    /// A podcast feed.
    Podcast,
    /// A podcast episode.
    Episode,
    /// An audiobook.
    Audiobook,
    /// A chapter in an audiobook.
    Chapter,
    /// A live radio station.
    Radio,
}

/// Stable composite identity. Remote IDs are never compared across sources.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentRef {
    /// Stable admitted resource/source identity.
    pub source_id: String,
    /// Provider-owned opaque media identity.
    pub remote_id: String,
    /// Kind of the referenced content.
    pub kind: ContentKind,
}

impl ContentRef {
    /// Construct an identity while rejecting blank locator components.
    pub fn new(
        source_id: impl Into<String>,
        remote_id: impl Into<String>,
        kind: ContentKind,
    ) -> Option<Self> {
        let source_id = source_id.into();
        let remote_id = remote_id.into();
        (!source_id.trim().is_empty() && !remote_id.trim().is_empty()).then_some(Self {
            source_id,
            remote_id,
            kind,
        })
    }
}

/// One playable source variant retained below a merged catalog item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVariant {
    /// Composite playable identity.
    pub content: ContentRef,
    /// Whether the bytes are already locally available.
    pub cached: bool,
    /// Whether the admitted source currently answers.
    pub reachable: bool,
    /// Operator source preference; larger values win.
    pub operator_priority: u32,
    /// Last measured request latency.
    pub latency_ms: Option<u32>,
}

/// A source-merged item shown in Home, Search, and Library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItem {
    /// Stable UI identity, chosen from normalized metadata rather than a remote ID.
    pub id: String,
    /// Kind of item.
    pub kind: ContentKind,
    /// Display title.
    pub title: String,
    /// Primary creator, when supplied.
    pub creator: String,
    /// Parent album/feed/book title, when supplied.
    pub parent_title: String,
    /// Duration in milliseconds, when finite.
    pub duration_ms: Option<u64>,
    /// Optional artwork identity/path reference.
    pub artwork_ref: Option<String>,
    /// Whether this item is starred in at least one admitted source.
    pub starred: bool,
    /// Whether one retained variant is locally cached.
    pub cached: bool,
    /// All source variants for explicit playback fallback.
    pub variants: Vec<SourceVariant>,
}

/// One provider bookmark projected into the workspace shelf.
///
/// Unlike a [`CatalogItem`], a bookmark retains the finite resume position so
/// a player surface can resume the provider item without inventing a second
/// bookmark authority. The content identity remains source-qualified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkItem {
    /// Provider-qualified media identity.
    pub content: ContentRef,
    /// Display title copied from the provider's bookmark entry.
    pub title: String,
    /// Primary creator, when supplied.
    pub creator: String,
    /// Parent feed, album, or book title, when supplied.
    pub parent_title: String,
    /// Resume position in milliseconds.
    pub position_ms: u64,
    /// Media duration in milliseconds, when finite and supplied.
    pub duration_ms: Option<u64>,
    /// Optional provider artwork identity.
    pub artwork_ref: Option<String>,
}

/// A Home shelf with no fake placeholders: an absent/empty shelf is omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeShelf {
    /// Stable shelf key.
    pub key: String,
    /// Human-readable heading.
    pub title: String,
    /// Ordered items.
    pub items: Vec<CatalogItem>,
}

/// One progressive search page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    /// Monotonic request generation issued by the UI.
    pub generation: u64,
    /// Query echoed for diagnostics and stale-result checks.
    pub query: String,
    /// Results grouped by kind in stable order.
    pub groups: BTreeMap<ContentKind, Vec<CatalogItem>>,
    /// Whether more source pages are expected.
    pub has_more: bool,
}

/// A named library collection and its supported presentation modes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryCollection {
    /// Stable collection key.
    pub key: String,
    /// Display name.
    pub title: String,
    /// Content kind represented by this collection.
    pub kind: ContentKind,
    /// Items in the current sort/filter window.
    pub items: Vec<CatalogItem>,
    /// Whether the source supports mutation of this collection.
    pub mutable: bool,
    /// Zero for a legacy/unpaged collection; otherwise the provider page offset.
    #[serde(default)]
    pub offset: usize,
    /// Zero for a legacy/unpaged collection; otherwise the requested page size.
    #[serde(default)]
    pub page_size: usize,
    /// Whether another page can be requested for this collection.
    #[serde(default)]
    pub has_more: bool,
}

/// Snapshot of the actual daemon transport state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    /// Current composite content identity.
    pub current: Option<ContentRef>,
    /// Whether the engine is playing.
    pub playing: bool,
    /// Current playhead in milliseconds.
    pub position_ms: u64,
    /// Duration, when seekable.
    pub duration_ms: Option<u64>,
    /// Normalized volume in thousandths.
    pub volume_milli: u16,
    /// Shuffle mode.
    pub shuffle: bool,
    /// Repeat mode: `off`, `context`, or `track`.
    pub repeat: String,
    /// Queue revision observed with this state.
    pub queue_revision: u64,
    /// Whether the current track can be sought.
    pub seekable: bool,
}

/// One queue entry with an explicit composite identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// Queue-stable entry identity.
    pub id: String,
    /// Composite item identity.
    pub content: ContentRef,
    /// Optional presentation title cached with the queue.
    pub title: String,
}

/// Managed offline download lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRecord {
    /// Composite content identity.
    pub content: ContentRef,
    /// `queued`, `downloading`, `ready`, `failed`, or `cancelled`.
    pub state: String,
    /// Downloaded bytes.
    pub bytes: u64,
    /// Expected bytes, when known.
    pub total_bytes: Option<u64>,
    /// Pinned downloads are retained during eviction.
    pub pinned: bool,
    /// Redacted diagnostic code on failure.
    pub error_code: Option<String>,
}

/// Durable audio-cache usage projected into the workspace snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicStorageSnapshot {
    /// Bytes currently indexed in the local audio cache.
    pub used_bytes: u64,
    /// Active cache cap applied by the daemon's GC policy.
    pub cap_bytes: u64,
}

/// A reachable local seat or admitted renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackTarget {
    /// Opaque target identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `local_seat`, `mesh_seat`, or `dlna_renderer`.
    pub kind: String,
    /// Whether the target may currently receive a handoff.
    pub available: bool,
    /// Human-readable unavailable reason, if any.
    pub unavailable_reason: Option<String>,
}

/// Capabilities discovered through the typed source adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Source identity.
    pub source_id: String,
    /// API profile/version observed.
    pub api_profile: String,
    /// Whether the source is reachable.
    pub reachable: bool,
    /// Whether auth is still required.
    pub authentication_required: bool,
    /// Supported typed feature names.
    pub features: BTreeSet<String>,
}

/// Complete retained workspace read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicWorkspaceSnapshotV1 {
    /// Contract discriminator.
    pub schema_version: u16,
    /// Snapshot revision, increasing on every publish.
    pub revision: u64,
    /// Home shelves with unsupported/empty shelves omitted.
    pub shelves: Vec<HomeShelf>,
    /// Typed bookmark shelf with resume positions retained.
    #[serde(default)]
    pub bookmarks: Vec<BookmarkItem>,
    /// Admitted library collections.
    pub collections: Vec<LibraryCollection>,
    /// Progressive search result, if a query is active.
    pub search: Option<SearchPage>,
    /// Current playback.
    pub playback: PlaybackSnapshot,
    /// Queue preview.
    pub queue: Vec<QueueEntry>,
    /// Managed downloads.
    pub downloads: Vec<DownloadRecord>,
    /// Local cache usage and the daemon-owned retention cap.
    #[serde(default)]
    pub storage: MusicStorageSnapshot,
    /// Available target seats/renderers.
    pub targets: Vec<PlaybackTarget>,
    /// Per-source capability truth.
    pub sources: Vec<ServerCapabilities>,
    /// Whether any source is currently reachable.
    pub any_source_reachable: bool,
}

/// Typed mutation accepted by the daemon's `action/music/*` lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicActionRequestV1 {
    /// Contract discriminator.
    pub schema_version: u16,
    /// Idempotency/replay guard.
    pub request_id: String,
    /// Typed action name.
    pub action: String,
    /// Optional content identity.
    #[serde(default)]
    pub content: Option<ContentRef>,
    /// Optional queue revision precondition.
    #[serde(default)]
    pub expected_queue_revision: Option<u64>,
    /// Optional position for seek/bookmark actions.
    #[serde(default)]
    pub position_ms: Option<u64>,
    /// Optional normalized output volume in thousandths (`0..=1000`).
    #[serde(default)]
    pub volume_milli: Option<u16>,
    /// Optional shuffle policy for the typed `shuffle` action.
    #[serde(default)]
    pub shuffle: Option<bool>,
    /// Optional repeat policy (`off`, `track`, or `context`) for `repeat`.
    #[serde(default)]
    pub repeat: Option<String>,
    /// Optional queue index for typed queue mutations.
    #[serde(default)]
    pub queue_index: Option<u16>,
    /// Optional destination queue index for a typed reorder.
    #[serde(default)]
    pub target_queue_index: Option<u16>,
    /// Peer that should receive a typed playback handoff.
    #[serde(default)]
    pub target_peer: Option<String>,
    /// Existing playlist identity for update/delete/reorder actions.
    #[serde(default)]
    pub playlist: Option<ContentRef>,
    /// New playlist name for create or rename update.
    #[serde(default)]
    pub playlist_name: Option<String>,
    /// Song ids to seed, add, or use as the complete reorder result.
    #[serde(default)]
    pub playlist_song_ids: Vec<String>,
    /// Existing playlist positions to remove during an update.
    #[serde(default)]
    pub playlist_remove_indices: Vec<u16>,
    /// One-use authorization material accepted at the wire boundary but never
    /// serialized into the retained request/result ledger.
    #[serde(default, skip_serializing)]
    pub armed_token: Option<String>,
}

/// Typed mutation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicActionResultV1 {
    /// Contract discriminator.
    pub schema_version: u16,
    /// Request id echoed for correlation.
    pub request_id: String,
    /// Whether the action was accepted.
    pub accepted: bool,
    /// New retained revision, when accepted.
    pub revision: u64,
    /// Redacted error code, when rejected.
    pub error_code: Option<String>,
}

impl MusicActionRequestV1 {
    /// Reject malformed or unbounded action envelopes before they reach a
    /// provider, queue, engine, or target adapter.
    ///
    /// # Errors
    ///
    /// Returns a stable redacted code describing the rejected contract field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != MUSIC_CONTRACT_VERSION {
            return Err("unsupported_schema_version");
        }
        if self.request_id.trim().is_empty()
            || self
                .request_id
                .chars()
                .any(|character| character.is_control())
        {
            return Err("missing_request_id");
        }
        if self.request_id.len() > MAX_REQUEST_ID_BYTES {
            return Err("request_id_too_large");
        }
        if !matches!(
            self.action.as_str(),
            "play"
                | "pause"
                | "resume"
                | "stop"
                | "next"
                | "previous"
                | "seek"
                | "scrobble"
                | "bookmark"
                | "bookmark_delete"
                | "set_volume"
                | "shuffle"
                | "repeat"
                | "star"
                | "unstar"
                | "playlist_create"
                | "playlist_update"
                | "playlist_delete"
                | "playlist_reorder"
                | "queue_move"
                | "queue_remove"
                | "queue_clear"
                | "download"
                | "cancel_download"
                | "remove_download"
                | "pin_download"
                | "unpin_download"
                | "transfer"
        ) {
            return Err("unknown_action");
        }
        if self
            .content
            .as_ref()
            .is_some_and(|content| !valid_content_ref(content))
        {
            return Err("invalid_content_ref");
        }
        if self.volume_milli.is_some_and(|volume| volume > 1000) {
            return Err("volume_out_of_range");
        }
        if self.target_peer.as_ref().is_some_and(|peer| {
            peer.trim().is_empty()
                || peer.len() > MAX_TARGET_PEER_BYTES
                || peer
                    .chars()
                    .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        }) {
            return Err("invalid_target_peer");
        }
        if self.action == "transfer" && self.target_peer.is_none() {
            return Err("missing_target_peer");
        }
        if matches!(self.action.as_str(), "seek" | "scrobble" | "bookmark")
            && self.position_ms.is_none()
        {
            return Err("missing_position");
        }
        if self.action == "set_volume" && self.volume_milli.is_none() {
            return Err("missing_volume");
        }
        if self.action == "shuffle" && self.shuffle.is_none() {
            return Err("missing_shuffle");
        }
        if self.action == "repeat" {
            let repeat = self.repeat.as_deref().ok_or("missing_repeat")?;
            if !matches!(repeat, "off" | "track" | "context") {
                return Err("invalid_repeat");
            }
        }
        if matches!(self.action.as_str(), "star" | "unstar") && self.content.is_none() {
            return Err("missing_content");
        }
        if matches!(
            self.action.as_str(),
            "scrobble" | "bookmark" | "bookmark_delete"
        ) && self.content.is_none()
        {
            return Err("missing_content");
        }
        if matches!(
            self.action.as_str(),
            "download" | "cancel_download" | "remove_download" | "pin_download" | "unpin_download"
        ) && self.content.is_none()
        {
            return Err("missing_content");
        }
        if self.playlist.as_ref().is_some_and(|playlist| {
            playlist.kind != ContentKind::Playlist || !valid_content_ref(playlist)
        }) {
            return Err("invalid_playlist_ref");
        }
        if self.playlist_song_ids.len() > MAX_PLAYLIST_ITEMS
            || self.playlist_remove_indices.len() > MAX_PLAYLIST_ITEMS
            || self.playlist_song_ids.iter().any(|id| {
                id.trim().is_empty()
                    || id.len() > MAX_PLAYLIST_FIELD_BYTES
                    || id.chars().any(char::is_control)
            })
        {
            return Err("playlist_items_too_large");
        }
        if self.playlist_name.as_ref().is_some_and(|name| {
            name.trim().is_empty()
                || name.len() > MAX_PLAYLIST_FIELD_BYTES
                || name.chars().any(char::is_control)
        }) {
            return Err("invalid_playlist_name");
        }
        match self.action.as_str() {
            "playlist_create" if self.playlist_name.is_none() => {
                return Err("missing_playlist_name");
            }
            "playlist_update" => {
                if self.playlist.is_none() {
                    return Err("missing_playlist");
                }
                if self.playlist_name.is_none()
                    && self.playlist_song_ids.is_empty()
                    && self.playlist_remove_indices.is_empty()
                {
                    return Err("missing_playlist_update");
                }
            }
            "playlist_delete" | "playlist_reorder" if self.playlist.is_none() => {
                return Err("missing_playlist");
            }
            _ => {}
        }
        if self
            .repeat
            .as_deref()
            .is_some_and(|repeat| repeat.len() > 16 || repeat.chars().any(char::is_control))
        {
            return Err("invalid_repeat");
        }
        Ok(())
    }
}

impl MusicWorkspaceSnapshotV1 {
    /// Validate retained data before publishing it to the shared Bus.
    ///
    /// # Errors
    ///
    /// Returns a stable redacted code when the snapshot version, bounds, or
    /// composite identities are invalid.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != MUSIC_CONTRACT_VERSION {
            return Err("unsupported_schema_version");
        }
        if self.revision == 0 {
            return Err("invalid_revision");
        }
        if self.shelves.len() > MAX_SHELVES
            || self.bookmarks.len() > MAX_BOOKMARKS
            || self.collections.len() > MAX_SOURCE_RECORDS
            || self.queue.len() > MAX_QUEUE_ITEMS
            || self.downloads.len() > MAX_QUEUE_ITEMS
            || self.targets.len() > MAX_SOURCE_RECORDS
            || self.sources.len() > MAX_SOURCE_RECORDS
        {
            return Err("collection_too_large");
        }
        if self
            .playback
            .current
            .as_ref()
            .is_some_and(|content| !valid_content_ref(content))
        {
            return Err("invalid_playback_identity");
        }
        let mut queue_ids = BTreeSet::new();
        if self.queue.iter().any(|entry| {
            entry.id.trim().is_empty()
                || entry.id.len() > MAX_QUEUE_ENTRY_ID_BYTES
                || entry.id.chars().any(char::is_control)
                || !valid_content_ref(&entry.content)
                || !queue_ids.insert(entry.id.as_str())
        }) {
            return Err("invalid_queue_identity");
        }
        if self
            .downloads
            .iter()
            .any(|entry| !valid_content_ref(&entry.content))
            || self
                .shelves
                .iter()
                .flat_map(|shelf| shelf.items.iter())
                .any(|item| !valid_catalog_item(item))
            || self
                .bookmarks
                .iter()
                .any(|bookmark| !valid_bookmark(bookmark))
            || self.collections.iter().any(|collection| {
                collection.items.len() > MAX_COLLECTION_ITEMS
                    || collection.page_size > MAX_LIBRARY_PAGE_SIZE
                    || (collection.page_size > 0 && collection.items.len() > collection.page_size)
                    || collection.offset > MAX_LIBRARY_OFFSET
            })
        {
            return Err("invalid_catalog_identity");
        }
        if self.storage.cap_bytes == 0 {
            return Err("invalid_storage_cap");
        }
        if self
            .search
            .as_ref()
            .is_some_and(|page| !accept_search_page(page.generation, &page.query, page))
        {
            return Err("invalid_search_page");
        }
        Ok(())
    }
}

fn valid_content_ref(content: &ContentRef) -> bool {
    !content.source_id.trim().is_empty() && !content.remote_id.trim().is_empty()
}

fn valid_catalog_item(item: &CatalogItem) -> bool {
    !item.id.trim().is_empty()
        && item.variants.len() <= MAX_SOURCE_VARIANTS
        && item
            .variants
            .iter()
            .all(|variant| valid_content_ref(&variant.content))
}

fn valid_bookmark(bookmark: &BookmarkItem) -> bool {
    valid_content_ref(&bookmark.content)
        && !bookmark.title.trim().is_empty()
        && bookmark
            .duration_ms
            .is_none_or(|duration| bookmark.position_ms <= duration)
}

/// Normalize metadata for cross-source matching.
#[must_use]
pub fn normalized_identity(
    kind: ContentKind,
    title: &str,
    creator: &str,
    parent: &str,
    duration_ms: Option<u64>,
) -> String {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "{kind:?}|{}|{}|{}|{}",
        normalize(title),
        normalize(creator),
        normalize(parent),
        duration_ms.unwrap_or_default()
    )
}

/// Merge variants by `MusicBrainz` id where available, otherwise normalized facts.
pub fn dedup_catalog(items: impl IntoIterator<Item = CatalogItem>) -> Vec<CatalogItem> {
    let mut merged: BTreeMap<String, CatalogItem> = BTreeMap::new();
    for item in items {
        let key = item.id.clone();
        match merged.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(item);
            }
            Entry::Occupied(mut slot) => {
                let entry = slot.get_mut();
                let remaining = MAX_SOURCE_VARIANTS.saturating_sub(entry.variants.len());
                entry
                    .variants
                    .extend(item.variants.into_iter().take(remaining));
                entry.starred |= item.starred;
                entry.cached |= item.cached;
            }
        }
    }
    merged.into_values().collect()
}

/// Pick a playback variant by cache, reachability, operator priority, latency.
#[must_use]
pub fn select_variant(variants: &[SourceVariant]) -> Option<&SourceVariant> {
    ordered_variants(variants).into_iter().next()
}

/// Return admitted playback variants in the same deterministic order used by
/// [`select_variant`]. Keeping the whole ordered set lets the daemon retry a
/// failed source without changing the queue identity or exposing credentials.
#[must_use]
pub fn ordered_variants(variants: &[SourceVariant]) -> Vec<&SourceVariant> {
    let mut ordered: Vec<&SourceVariant> = variants
        .iter()
        .filter(|v| v.reachable || v.cached)
        .collect();
    ordered.sort_by(|a, b| {
        b.cached
            .cmp(&a.cached)
            .then_with(|| b.reachable.cmp(&a.reachable))
            .then_with(|| b.operator_priority.cmp(&a.operator_priority))
            .then_with(|| match (a.latency_ms, b.latency_ms) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });
    ordered
}

/// Build only evidence-backed shelves from the current catalog signals.
#[must_use]
pub fn build_shelves(
    items: &[CatalogItem],
    starred: &[CatalogItem],
    recently_played: &[CatalogItem],
) -> Vec<HomeShelf> {
    let mut shelves = Vec::new();
    let add = |shelves: &mut Vec<HomeShelf>, key: &str, title: &str, values: &[CatalogItem]| {
        if !values.is_empty() {
            shelves.push(HomeShelf {
                key: key.to_string(),
                title: title.to_string(),
                items: values.to_vec(),
            });
        }
    };
    add(
        &mut shelves,
        "recently_played",
        "Recently Played",
        recently_played,
    );
    add(&mut shelves, "starred", "Starred", starred);
    add(&mut shelves, "library", "Your Library", items);
    shelves
}

/// Accept a search response only when it belongs to the latest request.
#[must_use]
pub fn accept_search_page(latest_generation: u64, latest_query: &str, page: &SearchPage) -> bool {
    page.generation == latest_generation
        && page.query == latest_query
        && page.groups.values().map(Vec::len).sum::<usize>() <= MAX_SEARCH_ITEMS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(
        source: &str,
        id: &str,
        cached: bool,
        priority: u32,
        latency_ms: Option<u32>,
    ) -> SourceVariant {
        SourceVariant {
            content: ContentRef::new(source, id, ContentKind::Music).expect("identity"),
            cached,
            reachable: true,
            operator_priority: priority,
            latency_ms,
        }
    }

    fn item(id: &str, variants: Vec<SourceVariant>) -> CatalogItem {
        CatalogItem {
            id: id.to_string(),
            kind: ContentKind::Music,
            title: "Song".into(),
            creator: "Artist".into(),
            parent_title: "Album".into(),
            duration_ms: Some(1000),
            artwork_ref: None,
            starred: false,
            cached: variants.iter().any(|v| v.cached),
            variants,
        }
    }

    #[test]
    fn source_collisions_do_not_merge_remote_ids_without_a_shared_key() {
        let merged = dedup_catalog([
            item("a", vec![variant("one", "same", false, 0, None)]),
            item("b", vec![variant("two", "same", false, 0, None)]),
        ]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn variant_selection_prefers_cache_then_priority_then_latency() {
        let variants = [
            variant("slow", "a", false, 99, Some(1)),
            variant("cached", "b", true, 0, Some(100)),
        ];
        assert_eq!(
            select_variant(&variants)
                .expect("variant")
                .content
                .remote_id,
            "b"
        );
    }

    #[test]
    fn ordered_variants_retains_reachable_fallbacks_in_policy_order() {
        let variants = [
            variant("slow", "a", false, 99, Some(200)),
            variant("fast", "b", false, 0, Some(10)),
            variant("cached", "c", true, 0, Some(500)),
        ];
        let ordered = ordered_variants(&variants);
        assert_eq!(
            ordered
                .iter()
                .map(|variant| variant.content.remote_id.as_str())
                .collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
    }

    #[test]
    fn empty_shelves_are_omitted_and_stale_search_is_rejected() {
        let songs = vec![item("song", vec![])];
        assert_eq!(build_shelves(&songs, &[], &[]).len(), 1);
        let page = SearchPage {
            generation: 2,
            query: "new".into(),
            groups: BTreeMap::new(),
            has_more: false,
        };
        assert!(!accept_search_page(1, "old", &page));
        assert!(accept_search_page(2, "new", &page));
    }

    #[test]
    fn action_validation_rejects_unknown_or_malformed_requests() {
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "request-1".into(),
            action: "play".into(),
            content: Some(ContentRef::new("source", "song", ContentKind::Music).unwrap()),
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
        };
        assert!(request.validate().is_ok());
        request.action = "run_command".into();
        assert_eq!(request.validate(), Err("unknown_action"));
        request.action = "play".into();
        request.request_id.clear();
        assert_eq!(request.validate(), Err("missing_request_id"));
        request.request_id = "request-1".into();
        request.action = "seek".into();
        assert_eq!(request.validate(), Err("missing_position"));
        request.position_ms = Some(42);
        assert!(request.validate().is_ok());
        request.action = "bookmark".into();
        request.content = Some(ContentRef::new("source", "episode", ContentKind::Episode).unwrap());
        request.position_ms = None;
        assert_eq!(request.validate(), Err("missing_position"));
        request.position_ms = Some(42);
        assert!(request.validate().is_ok());
        request.action = "bookmark_delete".into();
        request.position_ms = None;
        assert!(request.validate().is_ok());
        request.action = "transfer".into();
        request.target_peer = None;
        assert_eq!(request.validate(), Err("missing_target_peer"));
        request.target_peer = Some("peer/with-slash".into());
        assert_eq!(request.validate(), Err("invalid_target_peer"));
        request.target_peer = Some("seat-15".into());
        assert!(request.validate().is_ok());
        request.action = "set_volume".into();
        request.position_ms = None;
        assert_eq!(request.validate(), Err("missing_volume"));
        request.volume_milli = Some(1001);
        assert_eq!(request.validate(), Err("volume_out_of_range"));
        request.volume_milli = None;
        request.action = "shuffle".into();
        assert_eq!(request.validate(), Err("missing_shuffle"));
        request.shuffle = Some(true);
        assert!(request.validate().is_ok());
        request.action = "repeat".into();
        assert_eq!(request.validate(), Err("missing_repeat"));
        request.repeat = Some("context".into());
        assert!(request.validate().is_ok());
        request.repeat = Some("loop-forever".into());
        assert_eq!(request.validate(), Err("invalid_repeat"));
        request.repeat = None;
        request.action = "star".into();
        request.content = None;
        assert_eq!(request.validate(), Err("missing_content"));
        request.action = "playlist_create".into();
        request.playlist_name = None;
        assert_eq!(request.validate(), Err("missing_playlist_name"));
        request.playlist_name = Some("Roadtrip".into());
        request.playlist_song_ids = vec!["song-1".into()];
        assert!(request.validate().is_ok());
        request.action = "playlist_update".into();
        assert_eq!(request.validate(), Err("missing_playlist"));
        request.playlist =
            Some(ContentRef::new("legacy", "playlist-1", ContentKind::Playlist).unwrap());
        request.playlist_name = None;
        request.playlist_song_ids.clear();
        assert_eq!(request.validate(), Err("missing_playlist_update"));
    }

    #[test]
    fn snapshot_validation_bounds_queue_and_rejects_bad_identity() {
        let content = ContentRef::new("source", "song", ContentKind::Music).unwrap();
        let mut snapshot = MusicWorkspaceSnapshotV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            revision: 1,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: Some(content.clone()),
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".into(),
                queue_revision: 1,
                seekable: false,
            },
            queue: vec![QueueEntry {
                id: "entry-1".into(),
                content,
                title: "Song".into(),
            }],
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        };
        assert!(snapshot.validate().is_ok());
        snapshot.revision = 0;
        assert_eq!(snapshot.validate(), Err("invalid_revision"));
        snapshot.revision = 1;
        snapshot.storage.cap_bytes = 0;
        assert_eq!(snapshot.validate(), Err("invalid_storage_cap"));
        snapshot.storage.cap_bytes = 1;
        snapshot.queue = (0..=MAX_QUEUE_ITEMS)
            .map(|index| QueueEntry {
                id: format!("entry-{index}"),
                content: ContentRef::new("source", format!("song-{index}"), ContentKind::Music)
                    .unwrap(),
                title: "Song".into(),
            })
            .collect();
        assert_eq!(snapshot.validate(), Err("collection_too_large"));
    }

    #[test]
    fn restarted_snapshot_cannot_adopt_malformed_current_provider_identity() {
        let snapshot = MusicWorkspaceSnapshotV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            revision: 1,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: Some(ContentRef {
                    source_id: "provider-a".into(),
                    remote_id: " \t".into(),
                    kind: ContentKind::Music,
                }),
                playing: true,
                position_ms: 42,
                duration_ms: Some(1_000),
                volume_milli: 1_000,
                shuffle: false,
                repeat: "off".into(),
                queue_revision: 7,
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
        };

        assert_eq!(snapshot.validate(), Err("invalid_playback_identity"));
    }

    #[test]
    fn equivocated_queue_entry_identity_cannot_select_track_by_order() {
        let first = ContentRef::new("source", "song-a", ContentKind::Music).unwrap();
        let second = ContentRef::new("source", "song-b", ContentKind::Music).unwrap();
        let snapshot = MusicWorkspaceSnapshotV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            revision: 1,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: Some(first.clone()),
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".into(),
                queue_revision: 1,
                seekable: false,
            },
            queue: vec![
                QueueEntry {
                    id: "shared-entry".into(),
                    content: first,
                    title: "First".into(),
                },
                QueueEntry {
                    id: "shared-entry".into(),
                    content: second,
                    title: "Substituted".into(),
                },
            ],
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        };

        assert_eq!(snapshot.validate(), Err("invalid_queue_identity"));
    }
}
