//! TRANSFERS-4 — durable recurring rsync sync pairs.
//!
//! A sync pair is a saved `source`/`dest`/schedule tuple. The daemon scheduler turns
//! each due pair into an ordinary [`TransferJob`] with [`Method::Rsync`], so recurring
//! mirrors reuse the same queue cap, ledger, progress, verify, and notification
//! machinery as a one-shot rsync transfer.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::job::{Method, TransferJob, TransferPolicy};
use super::now_ms;

/// A saved recurring rsync mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPair {
    /// Stable operator-facing id, used as the filename under `sync-pairs/`.
    pub id: String,
    /// rsync source path/spec.
    pub source: String,
    /// rsync destination path/spec.
    pub dest: String,
    /// Interval in seconds. Zero is normalized to one second on save.
    pub every_secs: u64,
    /// Per-job transfer policy copied onto each fired rsync job.
    #[serde(default)]
    pub policy: TransferPolicy,
    /// Disabled pairs remain saved but never fire.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Last successful enqueue time, in wall-clock epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_ms: Option<u64>,
    /// Wall-clock ms when the pair was created.
    pub created_ms: u64,
    /// Wall-clock ms of the last pair mutation or scheduler enqueue.
    pub updated_ms: u64,
}

impl SyncPair {
    /// Build a new enabled sync pair.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        source: impl Into<String>,
        dest: impl Into<String>,
        every_secs: u64,
        policy: TransferPolicy,
    ) -> Self {
        let now = now_ms();
        Self {
            id: id.into(),
            source: source.into(),
            dest: dest.into(),
            every_secs: every_secs.max(1),
            policy,
            enabled: true,
            last_fired_ms: None,
            created_ms: now,
            updated_ms: now,
        }
    }

    /// Is this pair due at `now`?
    #[must_use]
    pub fn due_at(&self, now: u64) -> bool {
        if !self.enabled {
            return false;
        }
        match self.last_fired_ms {
            None => true,
            Some(last) => now >= last.saturating_add(self.every_secs.max(1).saturating_mul(1000)),
        }
    }

    /// Mint the ordinary rsync transfer job fired by this pair.
    #[must_use]
    pub fn to_job(&self) -> TransferJob {
        TransferJob::new(
            self.source.clone(),
            self.dest.clone(),
            Method::Rsync,
            self.policy.clone(),
        )
    }

    fn normalize_for_save(&mut self) {
        self.id = self.id.trim().to_string();
        self.source = self.source.trim().to_string();
        self.dest = self.dest.trim().to_string();
        self.every_secs = self.every_secs.max(1);
        self.updated_ms = now_ms();
        if self.created_ms == 0 {
            self.created_ms = self.updated_ms;
        }
    }
}

fn default_enabled() -> bool {
    true
}

/// Keep a corrupt or hostile sync-pair record from becoming an unbounded JSON
/// allocation. The record schema is small; this leaves ample room for legacy
/// paths and policy fields while still making the persistence boundary finite.
const MAX_SYNC_PAIR_RECORD_BYTES: usize = 256 * 1024;

/// Read one sync-pair record from the descriptor that will actually be used.
/// Unix opens refuse a final symlink and a nonblocking descriptor prevents a
/// FIFO or other special file from stalling the scheduler. The metadata check
/// and bounded sentinel read then apply to that same inode.
fn read_record(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sync-pair record must not be a symlink",
            ));
        }
        std::fs::File::open(path)?
    };

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync-pair record is not a regular file",
        ));
    }
    let limit = MAX_SYNC_PAIR_RECORD_BYTES as u64;
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sync-pair record exceeds {MAX_SYNC_PAIR_RECORD_BYTES}-byte limit"),
        ));
    }

    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_SYNC_PAIR_RECORD_BYTES)
        .saturating_add(1)
        .min(MAX_SYNC_PAIR_RECORD_BYTES.saturating_add(1));
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SYNC_PAIR_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sync-pair record exceeds {MAX_SYNC_PAIR_RECORD_BYTES}-byte limit"),
        ));
    }
    Ok(bytes)
}

/// One-directory persistent store for sync pair records.
#[derive(Debug, Clone)]
pub struct SyncPairStore {
    dir: PathBuf,
}

impl SyncPairStore {
    /// Open/create the `sync-pairs/` directory under the transfer store root.
    ///
    /// # Errors
    /// Fails if the directory cannot be created.
    pub fn open(store_root: &Path) -> io::Result<Self> {
        let dir = store_root.join("sync-pairs");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Insert or replace a sync pair.
    ///
    /// # Errors
    /// Invalid ids/paths/schedules or IO/serialization failures.
    pub fn upsert(&self, pair: &SyncPair) -> io::Result<()> {
        let mut pair = pair.clone();
        pair.normalize_for_save();
        validate_pair(&pair)?;
        let body = serde_json::to_string_pretty(&pair)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.dir.join(format!(".{}.json.tmp", pair.id));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, self.path(&pair.id))
    }

    /// Remove a pair. Missing is idempotent.
    ///
    /// # Errors
    /// IO failures other than not found, or invalid ids.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        validate_id(id)?;
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Load one pair by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<SyncPair> {
        if validate_id(id).is_err() {
            return None;
        }
        let data = read_record(&self.path(id)).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Load all parseable pairs, sorted by id for deterministic scheduling/tests.
    #[must_use]
    pub fn load_all(&self) -> Vec<SyncPair> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            if let Ok(data) = read_record(&path) {
                if let Ok(pair) = serde_json::from_slice::<SyncPair>(&data) {
                    out.push(pair);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Stamp a pair after a successful enqueue.
    ///
    /// # Errors
    /// Missing pair or IO failures.
    pub fn mark_fired(&self, id: &str, fired_ms: u64) -> io::Result<()> {
        let mut pair = self.get(id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("sync pair `{id}` not found"),
            )
        })?;
        pair.last_fired_ms = Some(fired_ms);
        pair.updated_ms = fired_ms;
        self.upsert(&pair)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

fn validate_pair(pair: &SyncPair) -> io::Result<()> {
    validate_id(&pair.id)?;
    if pair.source.is_empty() || pair.dest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync pair requires source and destination",
        ));
    }
    if pair.source.as_bytes().contains(&0) || pair.dest.as_bytes().contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync pair rejects NUL bytes in source or destination",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> io::Result<()> {
    let id = id.trim();
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.len() > 120
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid sync pair id `{id}`"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_pair_store_round_trips_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        store
            .upsert(&SyncPair::new(
                "z-pair",
                "/z",
                "/dest-z",
                60,
                TransferPolicy::default(),
            ))
            .unwrap();
        store
            .upsert(&SyncPair::new(
                "a-pair",
                "/a",
                "/dest-a",
                1,
                TransferPolicy::default(),
            ))
            .unwrap();
        let all = store.load_all();
        assert_eq!(
            all.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["a-pair", "z-pair"]
        );
        assert_eq!(store.get("z-pair").unwrap().source, "/z");
        store.remove("z-pair").unwrap();
        assert!(store.get("z-pair").is_none());
    }

    #[test]
    fn due_at_honors_enabled_and_interval() {
        let mut pair = SyncPair::new("pair", "/src", "/dst", 15, TransferPolicy::default());
        assert!(pair.due_at(1000));
        pair.last_fired_ms = Some(10_000);
        assert!(!pair.due_at(24_999));
        assert!(pair.due_at(25_000));
        pair.enabled = false;
        assert!(!pair.due_at(50_000));
    }

    #[test]
    fn bad_sync_pair_ids_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        let err = store
            .upsert(&SyncPair::new(
                "../escape",
                "/src",
                "/dst",
                1,
                TransferPolicy::default(),
            ))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn oversized_records_are_skipped_before_json_materialization() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        let pair = SyncPair::new(
            "oversized",
            "/source",
            "/dest",
            1,
            TransferPolicy::default(),
        );
        let mut body = serde_json::to_vec(&pair).unwrap();
        assert!(body.len() < MAX_SYNC_PAIR_RECORD_BYTES);
        body.resize(MAX_SYNC_PAIR_RECORD_BYTES + 1, b' ');
        std::fs::write(store.path("oversized"), body).unwrap();

        assert!(store.get("oversized").is_none());
        assert!(store.load_all().is_empty());
    }

    #[test]
    fn non_regular_records_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        std::fs::create_dir(store.path("directory")).unwrap();

        assert!(store.get("directory").is_none());
        assert!(store.load_all().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_records_are_skipped_without_reading_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        let target = tmp.path().join("outside.json");
        let record = store.path("linked");
        std::fs::write(&target, br#"{"id":"linked"}"#).unwrap();
        symlink(&target, &record).unwrap();

        assert!(store.get("linked").is_none());
        assert!(store.load_all().is_empty());
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            r#"{"id":"linked"}"#
        );
    }
}
