//! The offline cache (MEDIA-11): download Jellyfin titles to a managed local
//! cache so they play with no server.
//!
//! A title is downloaded once — the untouched direct-play bytes, fetched through
//! [`JellyfinClient::download`](crate::JellyfinClient::download) over the same
//! [`HttpTransport`](crate::HttpTransport) seam the browse calls use — and stored
//! under a cache root with a JSON [`manifest`](OfflineCache::save). Playing offline
//! is then a plain [`local_path`](OfflineCache::local_path) the media player loads
//! (the existing `PlayPath` path); no negotiation, no network.
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

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::store::config_base;

/// The offline cache root, relative to the user config dir:
/// `<config>/mde/jellyfin/offline`.
pub const CACHE_DIR_REL: &str = "mde/jellyfin/offline";

/// The manifest file name inside the cache root.
pub const MANIFEST_NAME: &str = "manifest.json";

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

/// The persisted manifest: the set of cached entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    entries: Vec<CacheEntry>,
}

/// Why an offline-cache operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// A filesystem read/write failed.
    #[error("offline cache io error: {0}")]
    Io(String),
    /// The manifest was not valid JSON.
    #[error("offline cache manifest parse error: {0}")]
    Parse(String),
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
        self.get(item_id).is_some()
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
        self.get(item_id).map(|e| self.root.join(&e.file_name))
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
    /// [`CacheError::OverBudget`] when the title exceeds the budget, or
    /// [`CacheError::Io`] / [`CacheError::Parse`] on a filesystem / manifest failure.
    pub fn store(
        &mut self,
        req: &CacheRequest,
        bytes: &[u8],
        now: u64,
    ) -> Result<CacheEntry, CacheError> {
        let incoming = bytes.len() as u64;
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
        std::fs::write(&path, bytes).map_err(|e| CacheError::Io(e.to_string()))?;

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
        std::fs::write(self.manifest_path(), json).map_err(|e| CacheError::Io(e.to_string()))
    }
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
}
