//! AIR-7 (v6.1) — mesh-shared audio cache + LRU eviction.
//!
//! Streamed audio is written to
//! `~/.local/share/mde/music-cache/<song-id>.<suffix>` — under the
//! mesh-shared data dir, so a track cached on one peer replicates to the
//! others (play on peer A, then play it offline on peer B). An
//! `index.json` alongside tracks `(song-id, bytes, last-played-ts,
//! starred, suffix)` for LRU eviction against a settings-adjustable cap
//! (default 10 GB). Starred songs (`getStarred2`) are pinned — never
//! evicted.
//!
//! The eviction policy + index bookkeeping are pure functions
//! (`total_bytes`, `evict_plan`, `record_play`, `upsert`) so they're
//! fully unit-testable; the playback engine (AIR-5) populates the cache
//! during streaming, and `mde-musicd cache {status,gc}` is the operator/
//! maintenance entry point that exercises the index end-to-end.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default cache cap: 10 GiB (Q27 — settings-adjustable).
pub const DEFAULT_CAP_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// Version of the durable cache index envelope.
pub const CACHE_SCHEMA_VERSION: u16 = 1;

/// MUSIC-ART-SYNC — the communal cover-art cache on the QNM-Shared mount. mackesd
/// (root) provisions `<mount>/music/artwork` 0777; musicd reads-through /
/// writes-through it so cover art pulled by ANY node is reused mesh-wide (one
/// Airsonic fetch, every node references the same image — and art keeps working
/// when a node can't reach the server). Overridable for tests / non-standard
/// mounts via `MDE_MESH_ARTWORK_DIR`.
const ARTWORK_DIR_ENV: &str = "MDE_MESH_ARTWORK_DIR";
const DEFAULT_ARTWORK_DIR: &str = "/mnt/mesh-storage/music/artwork";
const METADATA_DIR_ENV: &str = "MDE_MUSIC_METADATA_CACHE_DIR";

/// The communal artwork dir IF it currently exists (the mount is up + mackesd
/// provisioned it). `None` → fall back to a direct Airsonic fetch (no sharing).
#[must_use]
pub fn artwork_dir() -> Option<PathBuf> {
    let dir = std::env::var(ARTWORK_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_ARTWORK_DIR));
    dir.is_dir().then_some(dir)
}

/// Sanitize a Subsonic coverArt id into a safe single-path-component filename
/// (ids look like `al-12017` / `12017` / `pl-3`; never trust them as paths).
#[must_use]
pub fn artwork_filename(cover_id: &str) -> String {
    let safe: String = cover_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "_".to_string()
    } else {
        safe
    }
}

/// Read cached cover-art bytes from the communal mesh cache, if present + the
/// file is non-empty. `None` → not cached (or no mount) → caller fetches.
#[must_use]
pub fn read_shared_artwork(cover_id: &str) -> Option<Vec<u8>> {
    let path = artwork_dir()?.join(artwork_filename(cover_id));
    let bytes = std::fs::read(path).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// Write pulled-down cover-art bytes to the communal mesh cache (best-effort; a
/// failure — no mount, read-only, race — just means no sharing this time).
/// Writes to a temp sibling + renames so a concurrent reader never sees a
/// half-written image.
pub fn write_shared_artwork(cover_id: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let Some(dir) = artwork_dir() else { return };
    let name = artwork_filename(cover_id);
    let tmp = dir.join(format!(".{name}.tmp"));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(name));
    }
}

/// Sanitize an opaque Airsonic id into a safe single path component. This is
/// intentionally shared by artwork, audio, and metadata helpers: Subsonic ids are
/// server-owned strings, not filesystem paths.
#[must_use]
pub fn safe_component(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "_".to_string()
    } else {
        safe
    }
}

/// One cached track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// On-disk size in bytes.
    pub bytes: u64,
    /// Epoch-ms of the last play (the LRU key).
    pub last_played_ms: u64,
    /// Pinned against eviction (the song is starred on the server).
    #[serde(default)]
    pub starred: bool,
    /// File suffix (`flac` / `mp3` / `opus` / …) — locates the file.
    #[serde(default)]
    pub suffix: String,
}

/// The cache index: `song-id → entry`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheIndex {
    #[serde(default)]
    /// Map of song-id → cache entry, sorted for deterministic JSON output.
    pub entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFileV1 {
    schema_version: u16,
    index: CacheIndex,
}

impl CacheIndex {
    /// Total bytes across every cached track.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.bytes).sum()
    }

    /// Insert or update a track (called when a stream finishes caching).
    pub fn upsert(&mut self, song_id: &str, bytes: u64, suffix: &str, now_ms: u64, starred: bool) {
        self.entries.insert(
            song_id.to_string(),
            CacheEntry {
                bytes,
                last_played_ms: now_ms,
                starred,
                suffix: suffix.to_string(),
            },
        );
    }

    /// Bump a track's last-played timestamp (resets its LRU position).
    /// No-op when the track isn't cached.
    pub fn record_play(&mut self, song_id: &str, now_ms: u64) {
        if let Some(e) = self.entries.get_mut(song_id) {
            e.last_played_ms = now_ms;
        }
    }

    /// Mark/unmark a track as starred (pinned).
    pub fn set_starred(&mut self, song_id: &str, starred: bool) {
        if let Some(e) = self.entries.get_mut(song_id) {
            e.starred = starred;
        }
    }

    /// Song-ids to evict to bring the cache to `cap_bytes`: evict the
    /// least-recently-played **non-starred** tracks first, stopping once
    /// the total fits. Returns empty when already under cap (or when only
    /// starred tracks remain — starred are never evicted even if that
    /// leaves the cache over cap).
    #[must_use]
    pub fn evict_plan(&self, cap_bytes: u64) -> Vec<String> {
        let mut total = self.total_bytes();
        if total <= cap_bytes {
            return Vec::new();
        }
        // Non-starred tracks, oldest-played first.
        let mut candidates: Vec<(&String, &CacheEntry)> =
            self.entries.iter().filter(|(_, e)| !e.starred).collect();
        candidates.sort_by_key(|(_, e)| e.last_played_ms);

        let mut plan = Vec::new();
        for (id, e) in candidates {
            if total <= cap_bytes {
                break;
            }
            plan.push(id.clone());
            total = total.saturating_sub(e.bytes);
        }
        plan
    }
}

/// The cache directory: `$HOME/.local/share/mde/music-cache/`.
#[must_use]
pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(".local/share/mde/music-cache")
}

/// Epoch milliseconds used for cache recency bookkeeping.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Safe on-disk file name for a cached audio track.
#[must_use]
pub fn track_filename(song_id: &str, suffix: &str) -> String {
    format!("{}.{}", safe_component(song_id), safe_component(suffix))
}

/// Path for a cached audio track entry.
#[must_use]
pub fn track_path(dir: &Path, song_id: &str, suffix: &str) -> PathBuf {
    dir.join(track_filename(song_id, suffix))
}

/// Read fully cached audio bytes for `song_id`, if the index points at a
/// non-empty local file. Successful reads bump the LRU timestamp so replaying a
/// recently-played cached track during an AirSonic outage keeps it hot.
#[must_use]
pub fn read_cached_track_bytes(dir: &Path, song_id: &str, now_ms: u64) -> Option<Vec<u8>> {
    let mut index = read_index(dir);
    let entry = index.entries.get(song_id)?.clone();
    let path = track_path(dir, song_id, &entry.suffix);
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    index.record_play(song_id, now_ms);
    let _ = write_index(dir, &index);
    Some(bytes)
}

/// Return the suffix for a fully cached track without reading its audio bytes
/// or changing its LRU timestamp. Playback uses this bounded probe to decide
/// whether a queue can be served during an Airsonic outage.
#[must_use]
pub fn cached_track_suffix(dir: &Path, song_id: &str) -> Option<String> {
    let index = read_index(dir);
    let entry = index.entries.get(song_id)?;
    if entry.bytes == 0 {
        return None;
    }
    let path = track_path(dir, song_id, &entry.suffix);
    let metadata = std::fs::metadata(path).ok()?;
    (metadata.is_file() && metadata.len() > 0).then(|| entry.suffix.clone())
}

/// Write a fully-fetched finite stream into the recently-played audio cache.
/// Temp-then-rename keeps readers from seeing partial audio if playback is
/// interrupted mid-write; index persistence is best-effort for the caller.
pub fn write_cached_track(
    dir: &Path,
    song_id: &str,
    suffix: &str,
    bytes: &[u8],
    now_ms: u64,
    starred: bool,
) -> std::io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let name = track_filename(song_id, suffix);
    let tmp = dir.join(format!(".{name}.tmp"));
    let path = dir.join(&name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    let mut index = read_index(dir);
    index.upsert(
        song_id,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        suffix,
        now_ms,
        starred,
    );
    write_index(dir, &index)
}

/// Remove one cached finite track and its index entry. Missing tracks are a
/// successful no-op so a durable download record can be removed after an
/// eviction or manual cache cleanup without manufacturing a failure.
pub fn remove_cached_track(dir: &Path, song_id: &str) -> std::io::Result<bool> {
    let mut index = read_index(dir);
    let Some(entry) = index.entries.remove(song_id) else {
        return Ok(false);
    };
    let path = track_path(dir, song_id, &entry.suffix);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_index(dir, &index)?;
    Ok(true)
}

/// Directory for durable Airsonic metadata replies. Tests can override it with
/// `MDE_MUSIC_METADATA_CACHE_DIR`; production keeps it below the music cache.
#[must_use]
pub fn metadata_cache_dir() -> PathBuf {
    std::env::var(METADATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| cache_dir().join("metadata"))
}

fn fnv1a64(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in parts {
        for b in part.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Separator so ["ab", "c"] and ["a", "bc"] do not collide trivially.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Stable file path for one metadata cache entry. `scope` should be the selected
/// Airsonic/gateway base URL so switching sources cannot accidentally replay a
/// different server's stale metadata.
#[must_use]
pub fn metadata_cache_path(dir: &Path, scope: &str, view: &str, extra_key: &str) -> PathBuf {
    let digest = fnv1a64(&[scope, view, extra_key]);
    dir.join(format!("{}-{digest:016x}.json", safe_component(view)))
}

/// Read a cached Airsonic metadata inner response.
#[must_use]
pub fn read_cached_metadata(dir: &Path, scope: &str, view: &str, extra_key: &str) -> Option<Value> {
    let path = metadata_cache_path(dir, scope, view, extra_key);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist an Airsonic metadata inner response for outage fallback.
pub fn write_cached_metadata(dir: &Path, scope: &str, view: &str, extra_key: &str, value: &Value) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = metadata_cache_path(dir, scope, view, extra_key);
    let tmp = path.with_extension("tmp");
    let Ok(json) = serde_json::to_string_pretty(value) else {
        return;
    };
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Path of the index file within `dir`.
#[must_use]
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

/// Read the index from `dir` (empty index when absent/malformed — the
/// cache is a rebuildable best-effort store, never a hard error).
#[must_use]
pub fn read_index(dir: &Path) -> CacheIndex {
    let Some(text) = std::fs::read_to_string(index_path(dir)).ok() else {
        return CacheIndex::default();
    };
    if let Ok(envelope) = serde_json::from_str::<CacheFileV1>(&text) {
        if envelope.schema_version == CACHE_SCHEMA_VERSION {
            return envelope.index;
        }
    }
    // Version-zero migration: preserve the old index and rewrite it on the next
    // cache operation; media bytes remain in their existing safe paths.
    serde_json::from_str(&text).unwrap_or_default()
}

/// Write the index to `dir`, creating it if needed.
///
/// # Errors
/// IO / serialization failures.
pub fn write_index(dir: &Path, index: &CacheIndex) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(&CacheFileV1 {
        schema_version: CACHE_SCHEMA_VERSION,
        index: index.clone(),
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_index_atomically_with(&index_path(dir), json.as_bytes(), |temporary, target| {
        std::fs::rename(temporary, target)
    })
}

/// Replace the cache index without exposing a truncated or partially written
/// manifest to playback, pinning, or eviction readers.
fn write_index_atomically_with<F>(path: &Path, bytes: &[u8], replace: F) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "music cache index path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let target_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("index.json");
    let temporary = parent.join(format!(".{target_name}.{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Run eviction against `dir`: compute the plan, delete each evicted
/// track's file + drop it from the index, then persist. Returns the
/// list of evicted song-ids.
///
/// # Errors
/// IO failures persisting the trimmed index.
pub fn run_gc(dir: &Path, cap_bytes: u64) -> std::io::Result<Vec<String>> {
    let mut index = read_index(dir);
    let plan = index.evict_plan(cap_bytes);
    for id in &plan {
        if let Some(entry) = index.entries.remove(id) {
            let file = track_path(dir, id, &entry.suffix);
            let _ = std::fs::remove_file(file); // best-effort; absent is fine.
        }
    }
    if !plan.is_empty() {
        write_index(dir, &index)?;
    }
    Ok(plan)
}

/// `du -sh`-style human size (powers of 1024, one decimal past KiB).
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn artwork_filename_sanitizes_unsafe_chars() {
        assert_eq!(artwork_filename("al-12017"), "al-12017");
        assert_eq!(artwork_filename("12017"), "12017");
        // Path separators / traversal collapse to underscores (no escape).
        assert_eq!(artwork_filename("../../etc/passwd"), "______etc_passwd");
        assert_eq!(artwork_filename("a b/c"), "a_b_c");
        assert_eq!(artwork_filename(""), "_");
        assert_eq!(safe_component("a/b&c.flac"), "a_b_c.flac");
    }

    #[test]
    fn shared_artwork_round_trip_and_absent_dir() {
        // ONE test owns the process-global env override (no parallel race).
        // Absent dir → no cache.
        std::env::set_var(super::ARTWORK_DIR_ENV, "/nonexistent-artwork-dir-xyz-123");
        assert!(artwork_dir().is_none());
        assert!(read_shared_artwork("any").is_none());
        write_shared_artwork("any", b"bytes"); // best-effort no-op, must not panic

        // Real dir → write-through then read-through round-trips.
        let dir = tempdir().expect("tmp");
        std::env::set_var(super::ARTWORK_DIR_ENV, dir.path());
        let bytes = b"\xff\xd8\xff\xe0JFIF-ish-bytes".to_vec();
        write_shared_artwork("al-99", &bytes);
        assert_eq!(read_shared_artwork("al-99"), Some(bytes));
        // A miss returns None; empty writes never poison the cache.
        assert_eq!(read_shared_artwork("al-missing"), None);
        write_shared_artwork("al-empty", &[]);
        assert_eq!(read_shared_artwork("al-empty"), None);
        std::env::remove_var(super::ARTWORK_DIR_ENV);
    }

    fn idx(rows: &[(&str, u64, u64, bool)]) -> CacheIndex {
        let mut i = CacheIndex::default();
        for (id, bytes, last, starred) in rows {
            i.upsert(id, *bytes, "flac", *last, *starred);
        }
        i
    }

    #[test]
    fn total_bytes_sums_entries() {
        let i = idx(&[("a", 100, 1, false), ("b", 250, 2, false)]);
        assert_eq!(i.total_bytes(), 350);
    }

    #[test]
    fn under_cap_evicts_nothing() {
        let i = idx(&[("a", 100, 1, false), ("b", 100, 2, false)]);
        assert!(i.evict_plan(1000).is_empty());
    }

    #[test]
    fn evicts_oldest_played_first_until_under_cap() {
        // 4 tracks @100 each = 400; cap 250 → must drop 150+ → evict the
        // two oldest (c@1, a@2) = 200, leaving 200 <= 250.
        let i = idx(&[
            ("a", 100, 2, false),
            ("b", 100, 9, false),
            ("c", 100, 1, false),
            ("d", 100, 8, false),
        ]);
        let plan = i.evict_plan(250);
        assert_eq!(plan, vec!["c".to_string(), "a".to_string()]);
    }

    #[test]
    fn starred_tracks_are_never_evicted() {
        // a (starred, old) + b (non-starred, newer); cap 50 forces
        // eviction but only b is eligible.
        let i = idx(&[("a", 100, 1, true), ("b", 100, 9, false)]);
        let plan = i.evict_plan(50);
        assert_eq!(plan, vec!["b".to_string()]);
        // Even if cap can't be met (only starred left), no starred evict.
        let only_starred = idx(&[("a", 100, 1, true)]);
        assert!(only_starred.evict_plan(10).is_empty());
    }

    #[test]
    fn record_play_resets_lru_position() {
        let mut i = idx(&[("a", 100, 1, false), ("b", 100, 2, false)]);
        // a is oldest → would be evicted first.
        assert_eq!(i.evict_plan(100), vec!["a".to_string()]);
        // Play a → now b is oldest.
        i.record_play("a", 5);
        assert_eq!(i.evict_plan(100), vec!["b".to_string()]);
    }

    #[test]
    fn index_round_trips_through_disk() {
        let dir = tempdir().unwrap();
        let i = idx(&[("a", 100, 1, false), ("s", 200, 2, true)]);
        write_index(dir.path(), &i).unwrap();
        assert_eq!(read_index(dir.path()), i);
    }

    #[test]
    fn failed_index_replace_preserves_last_good_cache_authority() {
        let dir = tempdir().unwrap();
        let path = index_path(dir.path());
        let old = idx(&[("cached-old", 100, 1, true)]);
        write_index(dir.path(), &old).unwrap();

        let replacement = serde_json::to_vec(&CacheFileV1 {
            schema_version: CACHE_SCHEMA_VERSION,
            index: idx(&[("cached-new", 200, 2, false)]),
        })
        .unwrap();
        let error = write_index_atomically_with(&path, &replacement, |_temporary, _target| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "injected cache index replacement failure",
            ))
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(read_index(dir.path()), old);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "failed replacement cleans its unique temporary file"
        );
    }

    #[test]
    fn read_index_absent_is_empty() {
        let dir = tempdir().unwrap();
        assert_eq!(read_index(dir.path()), CacheIndex::default());
    }

    #[test]
    fn run_gc_deletes_files_and_trims_index() {
        let dir = tempdir().unwrap();
        // Two cached files; index says 100+100; cap 100 evicts the older.
        std::fs::write(track_path(dir.path(), "a", "flac"), b"xxxx").unwrap();
        std::fs::write(track_path(dir.path(), "b", "flac"), b"yyyy").unwrap();
        let i = idx(&[("a", 100, 1, false), ("b", 100, 9, false)]);
        write_index(dir.path(), &i).unwrap();

        let evicted = run_gc(dir.path(), 100).unwrap();
        assert_eq!(evicted, vec!["a".to_string()]);
        assert!(!track_path(dir.path(), "a", "flac").exists());
        assert!(track_path(dir.path(), "b", "flac").exists());
        // Index trimmed + persisted.
        let back = read_index(dir.path());
        assert!(!back.entries.contains_key("a"));
        assert!(back.entries.contains_key("b"));
    }

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(10 * 1024 * 1024 * 1024), "10.0 GiB");
    }

    #[test]
    fn cached_track_round_trips_and_records_recent_play() {
        let dir = tempdir().unwrap();
        write_cached_track(dir.path(), "song/7", "flac", b"audio-bytes", 10, false).unwrap();
        assert!(track_path(dir.path(), "song/7", "flac").exists());
        assert_eq!(
            cached_track_suffix(dir.path(), "song/7"),
            Some("flac".to_string())
        );

        let bytes = read_cached_track_bytes(dir.path(), "song/7", 99).expect("cached audio");
        assert_eq!(bytes, b"audio-bytes");
        let index = read_index(dir.path());
        assert_eq!(
            index.entries.get("song/7").map(|e| e.last_played_ms),
            Some(99)
        );
        std::fs::remove_file(track_path(dir.path(), "song/7", "flac")).unwrap();
        assert_eq!(cached_track_suffix(dir.path(), "song/7"), None);
    }

    #[test]
    fn removing_cached_track_removes_bytes_and_index_entry() {
        let dir = tempdir().unwrap();
        write_cached_track(dir.path(), "song-8", "audio", b"bytes", 10, false).unwrap();
        assert!(remove_cached_track(dir.path(), "song-8").unwrap());
        assert!(!track_path(dir.path(), "song-8", "audio").exists());
        assert!(!read_index(dir.path()).entries.contains_key("song-8"));
        assert!(!remove_cached_track(dir.path(), "song-8").unwrap());
    }

    #[test]
    fn metadata_cache_keys_include_scope_and_body() {
        let dir = tempdir().unwrap();
        let value = serde_json::json!({"albumList2": {"album": [{"id": "a"}]}});
        write_cached_metadata(
            dir.path(),
            "http://gateway-a/mde/airsonic/source-a",
            "getAlbumList2",
            "type=recent&size=10",
            &value,
        );
        assert_eq!(
            read_cached_metadata(
                dir.path(),
                "http://gateway-a/mde/airsonic/source-a",
                "getAlbumList2",
                "type=recent&size=10"
            ),
            Some(value)
        );
        assert!(
            read_cached_metadata(
                dir.path(),
                "http://gateway-b/mde/airsonic/source-a",
                "getAlbumList2",
                "type=recent&size=10"
            )
            .is_none(),
            "a different gateway/source scope must not reuse stale metadata"
        );
        assert!(
            read_cached_metadata(
                dir.path(),
                "http://gateway-a/mde/airsonic/source-a",
                "getAlbumList2",
                "type=newest&size=10"
            )
            .is_none(),
            "different query params get a different cache entry"
        );
    }

    #[test]
    fn cache_dir_is_under_mesh_data() {
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(
            cache_dir(),
            Path::new("/home/tester/.local/share/mde/music-cache")
        );
    }
}
