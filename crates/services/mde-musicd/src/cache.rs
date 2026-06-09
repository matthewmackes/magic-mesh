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
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default cache cap: 10 GiB (Q27 — settings-adjustable).
pub const DEFAULT_CAP_BYTES: u64 = 10 * 1024 * 1024 * 1024;

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
    pub entries: BTreeMap<String, CacheEntry>,
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

/// Path of the index file within `dir`.
#[must_use]
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

/// Read the index from `dir` (empty index when absent/malformed — the
/// cache is a rebuildable best-effort store, never a hard error).
#[must_use]
pub fn read_index(dir: &Path) -> CacheIndex {
    std::fs::read_to_string(index_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the index to `dir`, creating it if needed.
///
/// # Errors
/// IO / serialization failures.
pub fn write_index(dir: &Path, index: &CacheIndex) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(index)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(index_path(dir), json)
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
            let file = dir.join(format!("{id}.{}", entry.suffix));
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
    fn read_index_absent_is_empty() {
        let dir = tempdir().unwrap();
        assert_eq!(read_index(dir.path()), CacheIndex::default());
    }

    #[test]
    fn run_gc_deletes_files_and_trims_index() {
        let dir = tempdir().unwrap();
        // Two cached files; index says 100+100; cap 100 evicts the older.
        std::fs::write(dir.path().join("a.flac"), b"xxxx").unwrap();
        std::fs::write(dir.path().join("b.flac"), b"yyyy").unwrap();
        let i = idx(&[("a", 100, 1, false), ("b", 100, 9, false)]);
        write_index(dir.path(), &i).unwrap();

        let evicted = run_gc(dir.path(), 100).unwrap();
        assert_eq!(evicted, vec!["a".to_string()]);
        assert!(!dir.path().join("a.flac").exists());
        assert!(dir.path().join("b.flac").exists());
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
    fn cache_dir_is_under_mesh_data() {
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(
            cache_dir(),
            Path::new("/home/tester/.local/share/mde/music-cache")
        );
    }
}
