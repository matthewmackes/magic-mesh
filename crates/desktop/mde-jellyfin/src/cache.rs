//! The offline/cache layer (MEDIA-11 + WL-FUNC-015): downloaded Jellyfin titles
//! and credential-free metadata snapshots.
//!
//! A title is downloaded once — the untouched direct-play bytes, fetched through
//! [`JellyfinClient::download`](crate::JellyfinClient::download) over the same
//! [`HttpTransport`](crate::HttpTransport) seam the browse calls use — and stored
//! under a cache root with a JSON [`manifest`](OfflineCache::save). Playing offline
//! is then a plain [`local_path`](OfflineCache::local_path) the media player loads
//! (the existing `PlayPath` path); no negotiation, no network.
//!
//! The [`MetadataCache`] is a smaller sibling for gateway outages: it persists the
//! last successful browse/recent rows, image tags used to rebuild artwork URLs, and
//! per-user resume state from [`BaseItemDto`](crate::BaseItemDto) without accepting
//! or storing access tokens, stream descriptors, passwords, or sealed credential
//! references. It lets the Media Workspace honestly render stale metadata while
//! still requiring a live gateway (or a downloaded title) for playback.
//!
//! # Lifecycle
//!
//! The manifest is the source of truth for what is cached; the fold that manages
//! it is pure + fixture-tested:
//!
//! - **add** — [`store`](OfflineCache::store) writes the bytes + registers a
//!   [`CacheEntry`], evicting first to stay under budget.
//! - **evict** — [`evict`](OfflineCache::evict) removes one entry (file + manifest).
//! - **size-budget** — a [`size_budget`](OfflineCache::size_budget) caps the total
//!   bytes; a store over budget evicts least-recently-used entries to fit
//!   ([`enforce_budget`](OfflineCache::enforce_budget)).
//! - **stale** — an optional [`max_age_secs`](OfflineCache::max_age_secs) marks
//!   entries older than the TTL stale; [`evict_stale`](OfflineCache::evict_stale)
//!   (run at each store) sweeps them.
//!
//! Cross-device LRU is honest: [`touch`](OfflineCache::touch) bumps an entry's
//! last-access on offline play, and the budget fold evicts the coldest first.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    client::{image_url, ImageQuery, ImageType},
    models::BaseItemDto,
    store::config_base,
    sync::resume_position_secs,
};

/// The offline cache root, relative to the user config dir:
/// `<config>/mde/jellyfin/offline`.
pub const CACHE_DIR_REL: &str = "mde/jellyfin/offline";

/// The manifest file name inside the cache root.
pub const MANIFEST_NAME: &str = "manifest.json";

/// The metadata snapshot cache root, relative to the user config dir:
/// `<config>/mde/jellyfin/metadata`.
pub const METADATA_CACHE_DIR_REL: &str = "mde/jellyfin/metadata";

/// The metadata snapshot manifest inside the metadata cache root.
pub const METADATA_MANIFEST_NAME: &str = "snapshots.json";

/// The default size budget when one is not overridden: 16 GiB.
pub const DEFAULT_SIZE_BUDGET_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// One cached title — the manifest row describing a downloaded file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The Jellyfin item GUID this file is a copy of (the cache key).
    pub item_id: String,
    /// The id of the server it was downloaded from (for display + isolation).
    pub server_id: String,
    /// The media-source id the bytes came from, if known.
    #[serde(default)]
    pub source_id: Option<String>,
    /// The title, for the offline list.
    pub title: String,
    /// The container extension of the stored file (`mkv`, `mp4`, …).
    pub container: String,
    /// The stored file's name, relative to the cache root.
    pub file_name: String,
    /// The stored file's size in bytes.
    pub byte_len: u64,
    /// SHA-256 of the untouched downloaded bytes. Legacy rows without a digest
    /// fail closed instead of treating a same-sized file as the cached title.
    #[serde(default)]
    pub content_sha256: String,
    /// When it was downloaded (unix seconds) — the staleness clock.
    pub added_at: u64,
    /// When it was last played (unix seconds) — the LRU clock.
    pub last_access: u64,
}

/// The inputs to caching one title (everything but the bytes + clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRequest {
    /// The Jellyfin item GUID (the cache key).
    pub item_id: String,
    /// The id of the server the title is downloaded from.
    pub server_id: String,
    /// The media-source id, if known.
    pub source_id: Option<String>,
    /// The display title.
    pub title: String,
    /// The container extension of the file.
    pub container: String,
}

/// A credential-free snapshot of the last successful metadata browse for one
/// Jellyfin source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataSnapshot {
    /// Mesh/source-store id this snapshot belongs to.
    pub source_id: String,
    /// Human display label for stale-cache status text.
    pub source_label: String,
    /// The endpoint used to build cached artwork URLs. This is a gateway/direct
    /// source URL, never a token-bearing URL.
    pub endpoint: String,
    /// When this snapshot was captured (unix seconds).
    pub cached_at: u64,
    /// Last successful item rows. These carry metadata, image tags, and optional
    /// user resume state, but media-source descriptors are stripped before
    /// persistence because Jellyfin can place token-bearing stream URLs there.
    #[serde(default)]
    pub items: Vec<BaseItemDto>,
}

impl MetadataSnapshot {
    /// Return an item row by id.
    #[must_use]
    pub fn item(&self, item_id: &str) -> Option<&BaseItemDto> {
        self.items.iter().find(|item| item.id == item_id)
    }

    /// Build a cache-stable artwork URL for an item from the stored endpoint and
    /// image tag. Returns `None` when the snapshot has no primary-image tag for
    /// the item, so callers do not fabricate artwork availability.
    #[must_use]
    pub fn primary_artwork_url(&self, item_id: &str, max_width: Option<u32>) -> Option<String> {
        let item = self.item(item_id)?;
        let tag = item.image_tags.get(ImageType::Primary.as_str())?.clone();
        Some(image_url(
            &self.endpoint,
            item_id,
            ImageType::Primary,
            &ImageQuery {
                tag: Some(tag),
                max_width,
                ..ImageQuery::default()
            },
        ))
    }

    /// The rows with a positive resume position — the cached Continue-Watching
    /// signal during a temporary gateway/upstream outage.
    #[must_use]
    pub fn resumable_items(&self) -> Vec<&BaseItemDto> {
        self.items
            .iter()
            .filter(|item| {
                item.user_data
                    .as_ref()
                    .and_then(resume_position_secs)
                    .is_some()
            })
            .collect()
    }
}

/// The persisted manifest: the set of cached entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    entries: Vec<CacheEntry>,
}

/// The persisted metadata-snapshot manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MetadataManifest {
    #[serde(default)]
    snapshots: Vec<MetadataSnapshot>,
}

/// Why an offline-cache operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// A zero-byte response cannot be played and must never become an offline
    /// cache entry, even when a caller bypasses [`JellyfinClient::download`].
    #[error("cannot cache empty media")]
    EmptyMedia,
    /// A filesystem read/write failed.
    #[error("offline cache io error: {0}")]
    Io(String),
    /// The manifest was not valid JSON.
    #[error("offline cache manifest parse error: {0}")]
    Parse(String),
    /// A metadata provider attempted to replace a newer snapshot, or supplied
    /// different content for a generation that was already admitted.
    #[error(
        "metadata snapshot replay for {source_id}: incoming generation {incoming} does not advance current generation {current}"
    )]
    MetadataReplay {
        /// Provider/source whose projection would have rolled back.
        source_id: String,
        /// Latest admitted snapshot generation.
        current: u64,
        /// Replayed or conflicting generation.
        incoming: u64,
    },
    /// A single title is larger than the whole size budget, so it can never fit.
    #[error("title is {size} bytes, larger than the {budget}-byte cache budget")]
    OverBudget {
        /// The title's size in bytes.
        size: u64,
        /// The configured budget in bytes.
        budget: u64,
    },
}

/// A managed local cache of downloaded Jellyfin titles (MEDIA-11).
///
/// Holds the cache root + the manifest + the eviction policy (size budget +
/// optional staleness TTL). The root is not touched until the first
/// [`store`](Self::store); construction is pure, so the controller can build one
/// with the default root and tests point it at a scratch dir with
/// [`with_root`](Self::with_root).
#[derive(Debug, Clone)]
pub struct OfflineCache {
    root: PathBuf,
    entries: Vec<CacheEntry>,
    size_budget: Option<u64>,
    max_age_secs: Option<u64>,
}

/// A persisted, credential-free metadata/artwork/resume snapshot cache.
///
/// Unlike [`OfflineCache`], this does not claim a playable offline copy. It only
/// lets the UI render stale-but-useful rows while the gateway/upstream is
/// temporarily unavailable. The write path deliberately accepts no token,
/// `credential_ref`, password, or authenticated request headers, and strips
/// `MediaSources` before persistence so cached rows cannot carry stream tokens or
/// masquerade as playable offline media.
#[derive(Debug, Clone)]
pub struct MetadataCache {
    root: PathBuf,
    snapshots: Vec<MetadataSnapshot>,
}

impl Default for OfflineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineCache {
    /// A cache rooted at the [`default_root`](Self::default_root) with the default
    /// size budget and no staleness TTL. Does no filesystem work.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root(Self::default_root())
    }

    /// A cache rooted at `root` (the tests point this at a scratch dir), with the
    /// default budget + no staleness TTL. Does no filesystem work.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: Vec::new(),
            size_budget: Some(DEFAULT_SIZE_BUDGET_BYTES),
            max_age_secs: None,
        }
    }

    /// Set the size budget in bytes (`None` = unbounded); builder form.
    #[must_use]
    pub const fn with_size_budget(mut self, bytes: Option<u64>) -> Self {
        self.size_budget = bytes;
        self
    }

    /// Set the staleness TTL in seconds (`None` = never stale); builder form.
    #[must_use]
    pub const fn with_max_age(mut self, secs: Option<u64>) -> Self {
        self.max_age_secs = secs;
        self
    }

    /// The default cache root: `<config dir>/mde/jellyfin/offline`.
    #[must_use]
    pub fn default_root() -> PathBuf {
        let mut root = config_base();
        for part in CACHE_DIR_REL.split('/') {
            root.push(part);
        }
        root
    }

    // ── read-only accessors ───────────────────────────────────────────────────

    /// The cache root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The cached entries (manifest rows).
    #[must_use]
    pub fn entries(&self) -> &[CacheEntry] {
        &self.entries
    }

    /// The entry for `item_id`, if cached.
    #[must_use]
    pub fn get(&self, item_id: &str) -> Option<&CacheEntry> {
        self.entries.iter().find(|e| e.item_id == item_id)
    }

    /// Whether `item_id` is available offline.
    #[must_use]
    pub fn contains(&self, item_id: &str) -> bool {
        self.get(item_id)
            .is_some_and(|entry| self.entry_file_is_intact(entry))
    }

    /// The total bytes currently held.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.byte_len).sum()
    }

    /// The size budget in bytes (`None` = unbounded).
    #[must_use]
    pub const fn size_budget(&self) -> Option<u64> {
        self.size_budget
    }

    /// The staleness TTL in seconds (`None` = never stale).
    #[must_use]
    pub const fn max_age_secs(&self) -> Option<u64> {
        self.max_age_secs
    }

    /// The absolute path of the cached file for `item_id`, if cached — the URL the
    /// offline player loads.
    #[must_use]
    pub fn local_path(&self, item_id: &str) -> Option<PathBuf> {
        self.get(item_id)
            .filter(|entry| self.entry_file_is_intact(entry))
            .map(|entry| self.root.join(&entry.file_name))
    }

    /// The manifest path (`<root>/manifest.json`).
    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_NAME)
    }

    // ── pure lifecycle folds ──────────────────────────────────────────────────

    /// The item ids of entries that are stale at `now` — older than
    /// [`max_age_secs`](Self::max_age_secs) since download. Empty when no TTL is set.
    #[must_use]
    pub fn stale_ids(&self, now: u64) -> Vec<String> {
        let Some(max_age) = self.max_age_secs else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|e| now.saturating_sub(e.added_at) > max_age)
            .map(|e| e.item_id.clone())
            .collect()
    }

    /// The item ids to evict — least-recently-used first — so that the current
    /// total plus `incoming` bytes fits the budget. Empty when unbounded or already
    /// fitting; an entry already cached for `keep` is never chosen (a re-download
    /// replaces itself, not evicts a peer).
    #[must_use]
    pub fn lru_eviction_plan(&self, incoming: u64, keep: Option<&str>) -> Vec<String> {
        let Some(budget) = self.size_budget else {
            return Vec::new();
        };
        // The bytes already present, excluding the entry we're about to replace.
        let held: u64 = self
            .entries
            .iter()
            .filter(|e| Some(e.item_id.as_str()) != keep)
            .map(|e| e.byte_len)
            .sum();
        if held + incoming <= budget {
            return Vec::new();
        }
        // Coldest first (oldest last_access), tie-broken by oldest download.
        let mut candidates: Vec<&CacheEntry> = self
            .entries
            .iter()
            .filter(|e| Some(e.item_id.as_str()) != keep)
            .collect();
        candidates.sort_by(|a, b| {
            a.last_access
                .cmp(&b.last_access)
                .then(a.added_at.cmp(&b.added_at))
        });
        let mut freed = 0_u64;
        let mut plan = Vec::new();
        let need = (held + incoming).saturating_sub(budget);
        for entry in candidates {
            if freed >= need {
                break;
            }
            freed += entry.byte_len;
            plan.push(entry.item_id.clone());
        }
        plan
    }

    // ── mutations (touch the filesystem + persist the manifest) ────────────────

    /// Download-and-store `bytes` for `req` at `now`: sweep stale entries, evict
    /// least-recently-used to make room, write the file, register the entry, and
    /// persist the manifest.
    ///
    /// A title larger than the whole budget is [`CacheError::OverBudget`] (it can
    /// never fit); re-storing an already-cached item replaces it in place.
    ///
    /// # Errors
    /// [`CacheError::EmptyMedia`] when `bytes` is empty,
    /// [`CacheError::OverBudget`] when the title exceeds the budget, or
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn store(
        &mut self,
        req: &CacheRequest,
        bytes: &[u8],
        now: u64,
    ) -> Result<CacheEntry, CacheError> {
        let incoming = bytes.len() as u64;
        if bytes.is_empty() {
            return Err(CacheError::EmptyMedia);
        }
        if let Some(budget) = self.size_budget {
            if incoming > budget {
                return Err(CacheError::OverBudget {
                    size: incoming,
                    budget,
                });
            }
        }

        // Sweep stale first, then make budget room (never evicting the item we are
        // (re-)storing).
        self.evict_stale(now)?;
        for id in self.lru_eviction_plan(incoming, Some(&req.item_id)) {
            self.evict(&id)?;
        }

        // Write the bytes under the root.
        let file_name = cache_file_name(req);
        let path = self.root.join(&file_name);
        std::fs::create_dir_all(&self.root).map_err(|e| CacheError::Io(e.to_string()))?;
        write_atomic(&path, bytes).map_err(|e| CacheError::Io(e.to_string()))?;

        // Upsert the manifest entry, preserving the download time on a replace but
        // refreshing last-access (a re-download is a use).
        let added_at = self
            .get(&req.item_id)
            .map_or(now, |existing| existing.added_at);
        let entry = CacheEntry {
            item_id: req.item_id.clone(),
            server_id: req.server_id.clone(),
            source_id: req.source_id.clone(),
            title: req.title.clone(),
            container: req.container.clone(),
            file_name,
            byte_len: incoming,
            content_sha256: sha256_hex(bytes),
            added_at,
            last_access: now,
        };
        if let Some(existing) = self.entries.iter_mut().find(|e| e.item_id == entry.item_id) {
            *existing = entry.clone();
        } else {
            self.entries.push(entry.clone());
        }
        self.persist()?;
        Ok(entry)
    }

    /// Evict one item — delete its file and drop the manifest row. Returns the
    /// removed entry, or `None` when it was not cached.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn evict(&mut self, item_id: &str) -> Result<Option<CacheEntry>, CacheError> {
        let Some(pos) = self.entries.iter().position(|e| e.item_id == item_id) else {
            return Ok(None);
        };
        let entry = self.entries.remove(pos);
        let path = self.root.join(&entry.file_name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CacheError::Io(e.to_string())),
        }
        self.persist()?;
        Ok(Some(entry))
    }

    /// Evict every entry stale at `now` ([`stale_ids`](Self::stale_ids)). Returns
    /// the removed entries.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn evict_stale(&mut self, now: u64) -> Result<Vec<CacheEntry>, CacheError> {
        let mut removed = Vec::new();
        for id in self.stale_ids(now) {
            if let Some(entry) = self.evict(&id)? {
                removed.push(entry);
            }
        }
        Ok(removed)
    }

    /// Evict least-recently-used entries until the current total plus `incoming`
    /// fits the budget ([`lru_eviction_plan`](Self::lru_eviction_plan)). Returns the
    /// removed entries.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn enforce_budget(&mut self, incoming: u64) -> Result<Vec<CacheEntry>, CacheError> {
        let mut removed = Vec::new();
        for id in self.lru_eviction_plan(incoming, None) {
            if let Some(entry) = self.evict(&id)? {
                removed.push(entry);
            }
        }
        Ok(removed)
    }

    /// Bump an entry's last-access to `now` (offline play) so the LRU budget fold
    /// keeps it warm; persists the manifest. Returns whether the item was cached.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a manifest failure.
    pub fn touch(&mut self, item_id: &str, now: u64) -> Result<bool, CacheError> {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.item_id == item_id) {
            entry.last_access = now;
            self.persist()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ── persistence ───────────────────────────────────────────────────────────

    /// Load a cache rooted at `root` from its manifest. A missing manifest is a
    /// first-run empty cache (not an error).
    ///
    /// # Errors
    /// [`CacheError::Io`] on a read failure, [`CacheError::Parse`] on bad JSON.
    pub fn load_from(root: impl Into<PathBuf>) -> Result<Self, CacheError> {
        let mut cache = Self::with_root(root);
        let path = cache.manifest_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let manifest: Manifest =
                    serde_json::from_str(&text).map_err(|e| CacheError::Parse(e.to_string()))?;
                cache.entries = manifest.entries;
                Ok(cache)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(cache),
            Err(e) => Err(CacheError::Io(e.to_string())),
        }
    }

    /// Write the manifest to `<root>/manifest.json` (creating the root).
    ///
    /// # Errors
    /// [`CacheError::Io`] on a write failure, [`CacheError::Parse`] if serialization
    /// fails.
    pub fn save(&self) -> Result<(), CacheError> {
        self.persist()
    }

    /// Persist the manifest, creating the root dir.
    fn persist(&self) -> Result<(), CacheError> {
        std::fs::create_dir_all(&self.root).map_err(|e| CacheError::Io(e.to_string()))?;
        let manifest = Manifest {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CacheError::Parse(e.to_string()))?;
        write_atomic(&self.manifest_path(), json.as_bytes())
            .map_err(|e| CacheError::Io(e.to_string()))
    }

    fn entry_file_is_intact(&self, entry: &CacheEntry) -> bool {
        if !safe_cache_file_name(&entry.file_name) {
            return false;
        }
        let path = self.root.join(&entry.file_name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        // A zero-byte file is never a playable media copy. Keep this invariant
        // here as well as in `store`: a hostile or stale manifest must not turn
        // an empty file into an offline fallback. Size alone is insufficient:
        // bind playback to the exact bytes admitted by `store`.
        metadata.file_type().is_file()
            && metadata.len() > 0
            && metadata.len() == entry.byte_len
            && is_sha256_hex(&entry.content_sha256)
            && sha256_file(&path).is_ok_and(|digest| digest == entry.content_sha256)
    }
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataCache {
    /// A cache rooted at [`default_root`](Self::default_root). Does no filesystem
    /// work until loaded/saved/stored.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root(Self::default_root())
    }

    /// A cache rooted at `root` (tests use a scratch dir). Does no filesystem work.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            snapshots: Vec::new(),
        }
    }

    /// The default metadata cache root: `<config dir>/mde/jellyfin/metadata`.
    #[must_use]
    pub fn default_root() -> PathBuf {
        let mut root = config_base();
        for part in METADATA_CACHE_DIR_REL.split('/') {
            root.push(part);
        }
        root
    }

    /// The cache root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The snapshot manifest path.
    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(METADATA_MANIFEST_NAME)
    }

    /// All retained snapshots.
    #[must_use]
    pub fn snapshots(&self) -> &[MetadataSnapshot] {
        &self.snapshots
    }

    /// The snapshot for `source_id`, if present.
    #[must_use]
    pub fn snapshot(&self, source_id: &str) -> Option<&MetadataSnapshot> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.source_id == source_id)
    }

    /// Upsert one source's latest metadata snapshot and persist the manifest.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn store_snapshot(
        &mut self,
        source_id: impl Into<String>,
        source_label: impl Into<String>,
        endpoint: impl Into<String>,
        items: Vec<BaseItemDto>,
        cached_at: u64,
    ) -> Result<MetadataSnapshot, CacheError> {
        let snapshot = MetadataSnapshot {
            source_id: source_id.into(),
            source_label: source_label.into(),
            endpoint: endpoint.into(),
            cached_at,
            items: items.into_iter().map(metadata_snapshot_item).collect(),
        };
        let mut next = self.snapshots.clone();
        if let Some(existing) = next
            .iter_mut()
            .find(|entry| entry.source_id == snapshot.source_id)
        {
            if snapshot.cached_at < existing.cached_at
                || (snapshot.cached_at == existing.cached_at && snapshot != *existing)
            {
                return Err(CacheError::MetadataReplay {
                    source_id: snapshot.source_id,
                    current: existing.cached_at,
                    incoming: snapshot.cached_at,
                });
            }
            if snapshot == *existing {
                return Ok(snapshot);
            }
            *existing = snapshot.clone();
        } else {
            next.push(snapshot.clone());
        }
        self.persist_snapshots(&next)?;
        self.snapshots = next;
        Ok(snapshot)
    }

    /// Load a metadata cache rooted at `root`. A missing manifest is a first-run
    /// empty cache.
    ///
    /// # Errors
    /// [`CacheError::Io`] on a read failure, [`CacheError::Parse`] on bad JSON.
    pub fn load_from(root: impl Into<PathBuf>) -> Result<Self, CacheError> {
        let mut cache = Self::with_root(root);
        let path = cache.manifest_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let manifest: MetadataManifest =
                    serde_json::from_str(&text).map_err(|e| CacheError::Parse(e.to_string()))?;
                cache.snapshots = manifest.snapshots;
                Ok(cache)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(cache),
            Err(e) => Err(CacheError::Io(e.to_string())),
        }
    }

    /// Persist the current metadata snapshot manifest.
    ///
    /// # Errors
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn save(&self) -> Result<(), CacheError> {
        self.persist()
    }

    fn persist(&self) -> Result<(), CacheError> {
        self.persist_snapshots(&self.snapshots)
    }

    fn persist_snapshots(&self, snapshots: &[MetadataSnapshot]) -> Result<(), CacheError> {
        std::fs::create_dir_all(&self.root).map_err(|e| CacheError::Io(e.to_string()))?;
        let manifest = MetadataManifest {
            snapshots: snapshots.to_vec(),
        };
        let json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CacheError::Parse(e.to_string()))?;
        write_atomic(&self.manifest_path(), json.as_bytes())
            .map_err(|e| CacheError::Io(e.to_string()))
    }
}

fn metadata_snapshot_item(mut item: BaseItemDto) -> BaseItemDto {
    item.media_sources.clear();
    item
}

/// The filesystem-safe file name for a cached title:
/// `<server-slug>_<item-slug>.<container-slug>`. Both ids are slugged so a base-URL
/// server id (e.g. `https://jelly.mesh:8096`) is a valid path component.
fn cache_file_name(req: &CacheRequest) -> String {
    let container = if req.container.trim().is_empty() {
        "bin".to_string()
    } else {
        slug(&req.container)
    };
    format!(
        "{}_{}.{}",
        slug(&req.server_id),
        slug(&req.item_id),
        container
    )
}

fn safe_cache_file_name(file_name: &str) -> bool {
    let components = Path::new(file_name).components().collect::<Vec<_>>();
    !file_name.is_empty() && components.len() == 1 && matches!(components[0], Component::Normal(_))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache"),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Reduce `s` to `[A-Za-z0-9_-]`, mapping every other byte to `_`.
fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest.finish()
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finish())
}

struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    used: usize,
    byte_len: u64,
}

impl Sha256 {
    const fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            used: 0,
            byte_len: 0,
        }
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.byte_len = self.byte_len.wrapping_add(bytes.len() as u64);
        if self.used != 0 {
            let take = (64 - self.used).min(bytes.len());
            self.block[self.used..self.used + take].copy_from_slice(&bytes[..take]);
            self.used += take;
            bytes = &bytes[take..];
            if self.used == 64 {
                sha256_compress(&mut self.state, &self.block);
                self.used = 0;
            }
        }
        while bytes.len() >= 64 {
            let block: &[u8; 64] = bytes[..64].try_into().expect("fixed SHA-256 block");
            sha256_compress(&mut self.state, block);
            bytes = &bytes[64..];
        }
        self.block[..bytes.len()].copy_from_slice(bytes);
        self.used = bytes.len();
    }

    fn finish(mut self) -> String {
        let bit_len = self.byte_len.wrapping_mul(8);
        self.block[self.used] = 0x80;
        self.used += 1;
        if self.used > 56 {
            self.block[self.used..].fill(0);
            sha256_compress(&mut self.state, &self.block);
            self.block = [0; 64];
        } else {
            self.block[self.used..56].fill(0);
        }
        self.block[56..].copy_from_slice(&bit_len.to_be_bytes());
        sha256_compress(&mut self.state, &self.block);
        self.state
            .iter()
            .map(|word| format!("{word:08x}"))
            .collect()
    }
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8; 64]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut words = [0_u32; 64];
    for (index, word) in words[..16].iter_mut().enumerate() {
        *word = u32::from_be_bytes(
            block[index * 4..index * 4 + 4]
                .try_into()
                .expect("four-byte SHA-256 word"),
        );
    }
    for index in 16..64 {
        let first = words[index - 15].rotate_right(7)
            ^ words[index - 15].rotate_right(18)
            ^ (words[index - 15] >> 3);
        let second = words[index - 2].rotate_right(17)
            ^ words[index - 2].rotate_right(19)
            ^ (words[index - 2] >> 10);
        words[index] = words[index - 16]
            .wrapping_add(first)
            .wrapping_add(words[index - 7])
            .wrapping_add(second);
    }
    let mut work = *state;
    for index in 0..64 {
        let sigma1 = work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
        let choose = (work[4] & work[5]) ^ (!work[4] & work[6]);
        let first = work[7]
            .wrapping_add(sigma1)
            .wrapping_add(choose)
            .wrapping_add(K[index])
            .wrapping_add(words[index]);
        let sigma0 = work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
        let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
        let second = sigma0.wrapping_add(majority);
        work = [
            first.wrapping_add(second),
            work[0],
            work[1],
            work[2],
            work[3].wrapping_add(first),
            work[4],
            work[5],
            work[6],
        ];
    }
    for (slot, value) in state.iter_mut().zip(work) {
        *slot = slot.wrapping_add(value);
    }
}

/// The current unix time in seconds (the clock the app passes as `now`); tests use
/// fixed values instead.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn req(item_id: &str, title: &str) -> CacheRequest {
        CacheRequest {
            item_id: item_id.into(),
            server_id: "https://jelly.mesh:8096".into(),
            source_id: Some(format!("src-{item_id}")),
            title: title.into(),
            container: "mkv".into(),
        }
    }

    #[test]
    fn store_writes_the_file_and_registers_an_entry() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path());
        let entry = cache
            .store(&req("m1", "Movie One"), b"MEDIA-BYTES", 100)
            .expect("store");
        assert_eq!(entry.byte_len, 11);
        assert_eq!(entry.added_at, 100);
        assert!(cache.contains("m1"));
        // The file is on disk under the root, holding the exact bytes.
        let path = cache.local_path("m1").expect("path");
        assert!(path.starts_with(dir.path()));
        assert_eq!(std::fs::read(&path).expect("read"), b"MEDIA-BYTES");
        // The server-URL id is slugged into a valid file name.
        assert!(!entry.file_name.contains('/') && !entry.file_name.contains(':'));
        let temporary_files = std::fs::read_dir(dir.path())
            .expect("cache dir")
            .filter_map(Result::ok)
            .filter(|item| item.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temporary_files, 0, "atomic writes leave no temporary files");
    }

    #[test]
    fn offline_availability_rejects_missing_or_truncated_files() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path());
        cache
            .store(&req("m1", "Movie One"), b"MEDIA-BYTES", 100)
            .expect("store");
        let path = cache.local_path("m1").expect("path");

        std::fs::write(&path, b"short").expect("truncate");
        assert!(!cache.contains("m1"));
        assert!(cache.local_path("m1").is_none());

        std::fs::remove_file(&path).expect("remove");
        assert!(!cache.contains("m1"));
        assert!(cache.local_path("m1").is_none());
    }

    #[test]
    fn same_sized_substituted_media_is_rejected_after_restart() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path());
        let entry = cache
            .store(&req("m1", "Movie One"), b"GOOD", 100)
            .expect("store admitted bytes");
        assert_eq!(
            entry.content_sha256,
            "278f14e96cc67489e5c0d6cebec8a2718fb158ec656fd41fed7ecd031cd472b2"
        );

        let path = dir.path().join(&entry.file_name);
        std::fs::write(&path, b"EVIL").expect("substitute same-sized bytes");
        assert_eq!(
            std::fs::metadata(&path).expect("metadata").len(),
            entry.byte_len
        );

        let reloaded = OfflineCache::load_from(dir.path()).expect("restart cache");
        assert!(!reloaded.contains("m1"));
        assert!(
            reloaded.local_path("m1").is_none(),
            "byte length must not authorize substituted offline media"
        );
    }

    #[test]
    fn cache_rejects_empty_media_before_touching_existing_entries() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path());
        cache
            .store(&req("m1", "Movie One"), b"MEDIA-BYTES", 100)
            .expect("store");

        let err = cache
            .store(&req("m1", "Movie One"), &[], 200)
            .expect_err("empty media must not be cached");
        assert!(matches!(err, CacheError::EmptyMedia));
        assert_eq!(cache.entries().len(), 1);
        assert_eq!(
            std::fs::read(cache.local_path("m1").expect("existing path")).expect("existing bytes"),
            b"MEDIA-BYTES"
        );
    }

    #[test]
    fn cache_manifest_names_cannot_escape_the_cache_root() {
        assert!(safe_cache_file_name("server_item.mkv"));
        assert!(!safe_cache_file_name("../outside.mkv"));
        assert!(!safe_cache_file_name("nested/item.mkv"));
    }

    #[test]
    fn evict_removes_the_file_and_the_entry() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path());
        cache.store(&req("m1", "One"), b"AAAA", 1).expect("store");
        let path = cache.local_path("m1").expect("path");
        assert!(path.exists());
        let removed = cache.evict("m1").expect("evict").expect("was cached");
        assert_eq!(removed.item_id, "m1");
        assert!(!cache.contains("m1"));
        assert!(!path.exists(), "the file is deleted on evict");
        // Evicting an absent item is a no-op, not an error.
        assert!(cache.evict("m1").expect("evict again").is_none());
    }

    #[test]
    fn size_budget_evicts_least_recently_used_to_fit() {
        let dir = tempdir().expect("tempdir");
        // Budget of 10 bytes. Three 4-byte titles won't all fit.
        let mut cache = OfflineCache::with_root(dir.path()).with_size_budget(Some(10));
        cache.store(&req("a", "A"), b"AAAA", 1).expect("a"); // added t=1
        cache.store(&req("b", "B"), b"BBBB", 2).expect("b"); // added t=2
                                                             // Warm "a" so "b" is the coldest.
        cache.touch("a", 5).expect("touch a");
        // Storing "c" (4 bytes) needs 12 > 10 → evict the coldest ("b").
        cache.store(&req("c", "C"), b"CCCC", 6).expect("c");
        assert!(cache.contains("a"), "a was recently accessed");
        assert!(cache.contains("c"), "c is the new title");
        assert!(!cache.contains("b"), "b was the LRU victim");
        assert!(cache.total_bytes() <= 10);
        assert!(
            cache.local_path("b").is_none(),
            "the evicted title has no path"
        );
    }

    #[test]
    fn a_title_larger_than_the_whole_budget_is_rejected() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path()).with_size_budget(Some(4));
        let err = cache
            .store(&req("big", "Big"), b"TOO-LARGE", 1)
            .expect_err("over budget");
        assert!(matches!(err, CacheError::OverBudget { size: 9, budget: 4 }));
        assert!(!cache.contains("big"));
    }

    #[test]
    fn re_storing_replaces_in_place_without_evicting_a_peer() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path()).with_size_budget(Some(10));
        cache.store(&req("a", "A"), b"AAAA", 1).expect("a");
        cache.store(&req("b", "B"), b"BBBB", 2).expect("b");
        // Re-download "a" at the same size — must not evict "b" to make room.
        let entry = cache.store(&req("a", "A"), b"AZAZ", 9).expect("re-store a");
        assert_eq!(entry.added_at, 1, "download time is preserved on replace");
        assert_eq!(entry.last_access, 9, "last-access is refreshed");
        assert!(cache.contains("a") && cache.contains("b"));
        assert_eq!(cache.entries().len(), 2);
    }

    #[test]
    fn stale_entries_are_swept_on_the_next_store() {
        let dir = tempdir().expect("tempdir");
        // Entries older than 100s are stale.
        let mut cache = OfflineCache::with_root(dir.path()).with_max_age(Some(100));
        cache.store(&req("old", "Old"), b"OLD", 0).expect("old");
        assert_eq!(cache.stale_ids(50), Vec::<String>::new(), "fresh at t=50");
        // At t=200 the entry is stale; a new store sweeps it.
        assert_eq!(cache.stale_ids(200), vec!["old".to_string()]);
        cache.store(&req("new", "New"), b"NEW", 200).expect("new");
        assert!(!cache.contains("old"), "stale entry swept on store");
        assert!(cache.contains("new"));
    }

    #[test]
    fn evict_stale_is_a_noop_without_a_ttl() {
        let dir = tempdir().expect("tempdir");
        let mut cache = OfflineCache::with_root(dir.path()); // no max_age
        cache.store(&req("a", "A"), b"AAAA", 0).expect("a");
        let removed = cache.evict_stale(u64::MAX).expect("evict stale");
        assert!(removed.is_empty());
        assert!(cache.contains("a"));
    }

    #[test]
    fn manifest_round_trips_and_reload_sees_the_entries() {
        let dir = tempdir().expect("tempdir");
        {
            let mut cache = OfflineCache::with_root(dir.path());
            cache.store(&req("m1", "One"), b"AAAA", 1).expect("m1");
            cache.store(&req("m2", "Two"), b"BBBBBB", 2).expect("m2");
        }
        // A fresh cache over the same root reloads the manifest.
        let reloaded = OfflineCache::load_from(dir.path()).expect("load");
        assert_eq!(reloaded.entries().len(), 2);
        assert!(reloaded.contains("m1") && reloaded.contains("m2"));
        assert_eq!(reloaded.total_bytes(), 10);
        assert_eq!(reloaded.get("m2").expect("m2").title, "Two");
    }

    #[test]
    fn load_from_a_pristine_root_is_an_empty_cache() {
        let dir = tempdir().expect("tempdir");
        let cache = OfflineCache::load_from(dir.path()).expect("load pristine");
        assert!(cache.entries().is_empty());
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn default_root_lives_under_the_config_tree() {
        let root = OfflineCache::default_root();
        assert!(
            root.ends_with("mde/jellyfin/offline"),
            "got {}",
            root.display()
        );
    }

    fn metadata_item() -> BaseItemDto {
        let mut image_tags = std::collections::BTreeMap::new();
        image_tags.insert("Primary".to_string(), "poster-tag-1".to_string());
        BaseItemDto {
            id: "m1".to_string(),
            name: Some("Movie One".to_string()),
            item_type: Some("Movie".to_string()),
            overview: Some("Cached synopsis".to_string()),
            image_tags,
            media_sources: vec![crate::MediaSourceInfo {
                path: Some("http://gateway-a.mesh/stream?api_key=TOKEN".to_string()),
                transcoding_url: Some("/Videos/m1/main.m3u8?api_key=TOKEN".to_string()),
                ..crate::MediaSourceInfo::default()
            }],
            user_data: Some(crate::UserData {
                playback_position_ticks: 42 * crate::TICKS_PER_SECOND,
                ..crate::UserData::default()
            }),
            ..BaseItemDto::default()
        }
    }

    #[test]
    fn metadata_cache_persists_artwork_and_recent_rows_without_credentials() {
        let dir = tempdir().expect("tempdir");
        let mut cache = MetadataCache::with_root(dir.path());
        cache
            .store_snapshot(
                "gateway-a",
                "Gateway A",
                "http://gateway-a.mesh:8097/mde/jellyfin/gateway-a",
                vec![metadata_item()],
                1234,
            )
            .expect("store");

        let manifest = std::fs::read_to_string(cache.manifest_path()).expect("manifest");
        assert!(!manifest.contains("TOKEN"));
        assert!(!manifest.contains("api_key"));
        assert!(!manifest.contains("TranscodingUrl"));
        assert!(!manifest.contains("credential_ref"));
        assert!(!manifest.contains("media/jellyfin/shared-readonly"));

        let reloaded = MetadataCache::load_from(dir.path()).expect("reload");
        let snapshot = reloaded.snapshot("gateway-a").expect("snapshot");
        assert_eq!(snapshot.cached_at, 1234);
        assert_eq!(
            snapshot.items[0].overview.as_deref(),
            Some("Cached synopsis")
        );
        assert!(snapshot.items[0].media_sources.is_empty());
        assert_eq!(snapshot.resumable_items()[0].id, "m1");

        let artwork = snapshot
            .primary_artwork_url("m1", Some(320))
            .expect("primary artwork");
        assert_eq!(
            artwork,
            "http://gateway-a.mesh:8097/mde/jellyfin/gateway-a/Items/m1/Images/Primary?tag=poster-tag-1&maxWidth=320"
        );
        assert!(!artwork.contains("api_key"));
        assert!(!artwork.contains("TOKEN"));
    }

    #[test]
    fn metadata_cache_replaces_one_sources_snapshot_without_touching_peers() {
        let dir = tempdir().expect("tempdir");
        let mut cache = MetadataCache::with_root(dir.path());
        cache
            .store_snapshot("gateway-a", "Gateway A", "http://gateway-a.mesh", vec![], 1)
            .expect("a");
        cache
            .store_snapshot(
                "gateway-b",
                "Gateway B",
                "http://gateway-b.mesh",
                vec![metadata_item()],
                2,
            )
            .expect("b");
        cache
            .store_snapshot(
                "gateway-a",
                "Gateway A",
                "http://gateway-a.mesh",
                vec![metadata_item()],
                3,
            )
            .expect("replace a");

        assert_eq!(cache.snapshots().len(), 2);
        assert_eq!(cache.snapshot("gateway-a").expect("a").cached_at, 3);
        assert_eq!(cache.snapshot("gateway-b").expect("b").cached_at, 2);
    }

    #[test]
    fn metadata_replay_cannot_replace_current_provider_generation() {
        let dir = tempdir().expect("tempdir");
        let mut cache = MetadataCache::with_root(dir.path());
        cache
            .store_snapshot(
                "gateway-a",
                "Current Provider",
                "http://current.mesh",
                vec![metadata_item()],
                20,
            )
            .expect("current snapshot");

        for (label, endpoint, generation) in [
            ("Retired Provider", "http://retired.mesh", 19),
            ("Conflicting Provider", "http://conflict.mesh", 20),
        ] {
            let error = cache
                .store_snapshot("gateway-a", label, endpoint, Vec::new(), generation)
                .expect_err("non-forward provider replay must fail closed");
            assert!(matches!(error, CacheError::MetadataReplay { .. }));
        }

        let current = cache.snapshot("gateway-a").expect("current projection");
        assert_eq!(current.cached_at, 20);
        assert_eq!(current.source_label, "Current Provider");
        assert_eq!(current.endpoint, "http://current.mesh");
        assert_eq!(current.items.len(), 1);

        let reloaded = MetadataCache::load_from(dir.path()).expect("reload current projection");
        assert_eq!(
            reloaded.snapshot("gateway-a").expect("persisted current"),
            current
        );
    }

    #[test]
    fn failed_metadata_snapshot_replacement_keeps_the_last_complete_projection() {
        let dir = tempdir().expect("tempdir");
        let mut cache = MetadataCache::with_root(dir.path());
        cache
            .store_snapshot(
                "gateway-a",
                "Gateway A",
                "http://gateway-a.mesh",
                vec![metadata_item()],
                1,
            )
            .expect("initial snapshot");

        let manifest = cache.manifest_path();
        let last_good = dir.path().join("last-good.json");
        std::fs::rename(&manifest, &last_good).expect("preserve manifest fixture");
        std::fs::create_dir(&manifest).expect("block atomic replacement");

        let error = cache
            .store_snapshot(
                "gateway-a",
                "Gateway A",
                "http://gateway-a.mesh",
                Vec::new(),
                2,
            )
            .expect_err("failed persistence must be reported");
        assert!(matches!(error, CacheError::Io(_)));
        assert_eq!(
            cache.snapshot("gateway-a").expect("last good").cached_at,
            1,
            "an unpersisted provider refresh must not replace the live fallback"
        );

        std::fs::remove_dir(&manifest).expect("remove blocker");
        std::fs::rename(&last_good, &manifest).expect("restore last good manifest");
        let reloaded = MetadataCache::load_from(dir.path()).expect("reload last good");
        assert_eq!(
            reloaded
                .snapshot("gateway-a")
                .expect("persisted last good")
                .cached_at,
            1
        );
        assert!(
            std::fs::read_dir(dir.path())
                .expect("cache root")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "failed atomic replacement must clean its temporary file"
        );
    }
}
