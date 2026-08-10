//! Bounded, integrity-checked disk cache for offline map tiles.
//!
//! This authority performs filesystem work only when explicitly called by a
//! worker. Rendering code receives owned verified bytes or an explicit miss;
//! it never downloads, scans, or validates files during a paint pass.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::offline_catalog::{is_sha256_hex, sha256_hex, TileId, VerifiedCatalog, MAX_TILE_BYTES};

const INDEX_SCHEMA: u16 = 2;
const INDEX_FILE: &str = "index.json";
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    pub quota_bytes: u64,
    pub max_age_ms: u64,
}

impl CachePolicy {
    pub fn bounded(quota_bytes: u64, max_age_ms: u64) -> Result<Self, CacheError> {
        if quota_bytes == 0 || max_age_ms == 0 {
            return Err(CacheError::Policy(
                "cache quota and max age must be non-zero",
            ));
        }
        Ok(Self {
            quota_bytes,
            max_age_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    Io(String),
    Index(String),
    CorruptIndex(String),
    Policy(&'static str),
    NotApproved,
    Digest,
    OverQuota,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cache I/O failed: {error}"),
            Self::Index(error) | Self::CorruptIndex(error) => {
                write!(f, "cache index is invalid: {error}")
            }
            Self::Policy(error) => f.write_str(error),
            Self::NotApproved => f.write_str("tile is not approved by the verified catalog"),
            Self::Digest => f.write_str("tile digest is malformed or does not match"),
            Self::OverQuota => f.write_str("tile cannot fit within the cache quota"),
        }
    }
}

impl std::error::Error for CacheError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnavailableReason {
    NotIndexed,
    CatalogRejectedRemoved,
    Expired,
    MissingRemoved,
    CorruptRemoved,
    CacheFailure(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineTile {
    Verified { bytes: Vec<u8>, sha256: String },
    Unavailable(UnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheEntry {
    tile: TileId,
    catalog_sha256: String,
    sha256: String,
    byte_len: u64,
    verified_at_ms: u64,
    last_access_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheIndex {
    schema: u16,
    entries: Vec<CacheEntry>,
}

#[derive(Debug, Deserialize)]
struct CacheIndexHeader {
    schema: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCacheEntry {
    tile: TileId,
    sha256: String,
    byte_len: u64,
    #[serde(rename = "verified_at_ms")]
    _verified_at_ms: u64,
    #[serde(rename = "last_access_ms")]
    _last_access_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCacheIndex {
    schema: u16,
    entries: Vec<LegacyCacheEntry>,
}

pub struct OfflineTileCache {
    root: PathBuf,
    policy: CachePolicy,
    entries: Vec<CacheEntry>,
}

impl OfflineTileCache {
    pub fn open(root: impl Into<PathBuf>, policy: CachePolicy) -> Result<Self, CacheError> {
        let root = root.into();
        ensure_root(&root)?;
        let entries = match load_index(&root, policy) {
            Ok(entries) => entries,
            Err(CacheError::CorruptIndex(_)) => recover_corrupt_regular_index(&root)?,
            Err(error) => return Err(error),
        };
        Ok(Self {
            root,
            policy,
            entries,
        })
    }

    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.byte_len).sum()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn store_verified(
        &mut self,
        catalog: &VerifiedCatalog,
        tile: TileId,
        bytes: &[u8],
        expected_sha256: &str,
        now_ms: u64,
    ) -> Result<(), CacheError> {
        if !catalog.permits(&tile, now_ms) {
            return Err(CacheError::NotApproved);
        }
        if bytes.is_empty() || bytes.len() > MAX_TILE_BYTES {
            return Err(CacheError::Policy("tile byte length is outside bounds"));
        }
        if !is_sha256_hex(expected_sha256) || sha256_hex(bytes) != expected_sha256 {
            return Err(CacheError::Digest);
        }
        let incoming = bytes.len() as u64;
        if incoming > self.policy.quota_bytes {
            return Err(CacheError::OverQuota);
        }
        self.remove_expired(now_ms)?;
        if let Some(position) = self.entries.iter().position(|entry| entry.tile == tile) {
            self.remove_position(position)?;
        }
        while self.used_bytes().saturating_add(incoming) > self.policy.quota_bytes {
            let position = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| (entry.last_access_ms, entry.verified_at_ms, &entry.tile))
                .map(|(position, _)| position)
                .ok_or(CacheError::OverQuota)?;
            self.remove_position(position)?;
        }

        let path = tile_path(&self.root, &tile, expected_sha256)?;
        ensure_tile_parent(&self.root, &tile)?;
        write_atomic(&path, bytes)?;
        self.entries.push(CacheEntry {
            tile,
            catalog_sha256: catalog.digest().to_string(),
            sha256: expected_sha256.to_string(),
            byte_len: incoming,
            verified_at_ms: now_ms,
            last_access_ms: now_ms,
        });
        if let Err(error) = self.persist() {
            self.entries.pop();
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        Ok(())
    }

    pub fn lookup(&mut self, catalog: &VerifiedCatalog, tile: &TileId, now_ms: u64) -> OfflineTile {
        let Some(position) = self.entries.iter().position(|entry| &entry.tile == tile) else {
            return OfflineTile::Unavailable(UnavailableReason::NotIndexed);
        };
        if self.entries[position].catalog_sha256 != catalog.digest()
            || !catalog.permits(tile, now_ms)
        {
            return match self.remove_position(position) {
                Ok(()) => OfflineTile::Unavailable(UnavailableReason::CatalogRejectedRemoved),
                Err(error) => {
                    OfflineTile::Unavailable(UnavailableReason::CacheFailure(error.to_string()))
                }
            };
        }
        if self.entries[position].verified_at_ms > now_ms
            || self.entries[position].last_access_ms > now_ms
        {
            return self.remove_bad(position, UnavailableReason::CorruptRemoved, None);
        }
        if now_ms.saturating_sub(self.entries[position].verified_at_ms) > self.policy.max_age_ms {
            return match self.remove_position(position) {
                Ok(()) => OfflineTile::Unavailable(UnavailableReason::Expired),
                Err(error) => {
                    OfflineTile::Unavailable(UnavailableReason::CacheFailure(error.to_string()))
                }
            };
        }
        let entry = self.entries[position].clone();
        let path = match tile_path(&self.root, &entry.tile, &entry.sha256) {
            Ok(path) => path,
            Err(error) => {
                return OfflineTile::Unavailable(UnavailableReason::CacheFailure(error.to_string()))
            }
        };
        let bytes = match read_bounded_regular_file(&path, entry.byte_len) {
            Ok(bytes) => bytes,
            Err(ReadFailure::Missing) => {
                return self.remove_bad(position, UnavailableReason::MissingRemoved, None)
            }
            Err(ReadFailure::UnsafeOrIo) => {
                return self.remove_bad(position, UnavailableReason::CorruptRemoved, Some(&path))
            }
        };
        if sha256_hex(&bytes) != entry.sha256 {
            return self.remove_bad(position, UnavailableReason::CorruptRemoved, Some(&path));
        }
        self.entries[position].last_access_ms = now_ms;
        if let Err(error) = self.persist() {
            return OfflineTile::Unavailable(UnavailableReason::CacheFailure(error.to_string()));
        }
        OfflineTile::Verified {
            bytes,
            sha256: entry.sha256,
        }
    }

    fn remove_expired(&mut self, now_ms: u64) -> Result<(), CacheError> {
        let mut positions: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                now_ms.saturating_sub(entry.verified_at_ms) > self.policy.max_age_ms
            })
            .map(|(position, _)| position)
            .collect();
        positions.reverse();
        for position in positions {
            self.remove_position(position)?;
        }
        Ok(())
    }

    fn remove_bad(
        &mut self,
        position: usize,
        reason: UnavailableReason,
        path: Option<&Path>,
    ) -> OfflineTile {
        if let Some(path) = path {
            quarantine_then_remove(path);
        }
        match self.remove_position(position) {
            Ok(()) => OfflineTile::Unavailable(reason),
            Err(error) => {
                OfflineTile::Unavailable(UnavailableReason::CacheFailure(error.to_string()))
            }
        }
    }

    fn remove_position(&mut self, position: usize) -> Result<(), CacheError> {
        let entry = self.entries.remove(position);
        let path = tile_path(&self.root, &entry.tile, &entry.sha256)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                self.entries.insert(position, entry);
                return Err(CacheError::Io(error.to_string()));
            }
        }
        if let Err(error) = self.persist() {
            self.entries.insert(position, entry);
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(&CacheIndex {
            schema: INDEX_SCHEMA,
            entries: self.entries.clone(),
        })
        .map_err(|error| CacheError::Index(error.to_string()))?;
        if bytes.len() as u64 > MAX_INDEX_BYTES {
            return Err(CacheError::Index(
                "serialized index exceeds byte bound".to_string(),
            ));
        }
        write_atomic(&self.root.join(INDEX_FILE), &bytes)
    }
}

enum ReadFailure {
    Missing,
    UnsafeOrIo,
}

fn read_bounded_regular_file(path: &Path, expected: u64) -> Result<Vec<u8>, ReadFailure> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadFailure::Missing
        } else {
            ReadFailure::UnsafeOrIo
        }
    })?;
    if !metadata.file_type().is_file()
        || metadata.len() != expected
        || expected > MAX_TILE_BYTES as u64
    {
        return Err(ReadFailure::UnsafeOrIo);
    }
    let file = File::open(path).map_err(|_| ReadFailure::UnsafeOrIo)?;
    #[cfg(unix)]
    {
        let opened = file.metadata().map_err(|_| ReadFailure::UnsafeOrIo)?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(ReadFailure::UnsafeOrIo);
        }
    }
    let mut bytes = Vec::with_capacity(expected as usize);
    file.take(expected + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadFailure::UnsafeOrIo)?;
    if bytes.len() as u64 != expected {
        return Err(ReadFailure::UnsafeOrIo);
    }
    Ok(bytes)
}

fn load_index(root: &Path, policy: CachePolicy) -> Result<Vec<CacheEntry>, CacheError> {
    let path = root.join(INDEX_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(CacheError::Io(error.to_string())),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_INDEX_BYTES {
        return Err(CacheError::Index(
            "index is not a bounded regular file".to_string(),
        ));
    }
    let file = File::open(&path).map_err(|error| CacheError::Io(error.to_string()))?;
    #[cfg(unix)]
    {
        let opened = file
            .metadata()
            .map_err(|error| CacheError::Io(error.to_string()))?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(CacheError::Index(
                "index changed while it was being opened".to_string(),
            ));
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CacheError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(CacheError::Index(
            "index exceeds its byte bound".to_string(),
        ));
    }
    let header: CacheIndexHeader = serde_json::from_slice(&bytes)
        .map_err(|error| CacheError::CorruptIndex(error.to_string()))?;
    if header.schema == 1 {
        return invalidate_legacy_index(root, &bytes);
    }
    if header.schema != INDEX_SCHEMA {
        return Err(CacheError::Index("index schema is unsupported".to_string()));
    }
    let index: CacheIndex = serde_json::from_slice(&bytes)
        .map_err(|error| CacheError::CorruptIndex(error.to_string()))?;
    if index.entries.len() > MAX_ENTRIES {
        return Err(CacheError::CorruptIndex(
            "index entry count is invalid".to_string(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut total = 0_u64;
    for entry in &index.entries {
        entry
            .tile
            .validate()
            .map_err(|error| CacheError::CorruptIndex(error.to_string()))?;
        if !is_sha256_hex(&entry.catalog_sha256)
            || !is_sha256_hex(&entry.sha256)
            || entry.byte_len == 0
            || entry.byte_len > MAX_TILE_BYTES as u64
            || !identities.insert(entry.tile.clone())
        {
            return Err(CacheError::CorruptIndex(
                "index entry is malformed or duplicated".to_string(),
            ));
        }
        total = total
            .checked_add(entry.byte_len)
            .ok_or_else(|| CacheError::CorruptIndex("index size overflow".to_string()))?;
    }
    if total > policy.quota_bytes {
        return Err(CacheError::Index(
            "index exceeds configured quota".to_string(),
        ));
    }
    Ok(index.entries)
}

fn invalidate_legacy_index(root: &Path, bytes: &[u8]) -> Result<Vec<CacheEntry>, CacheError> {
    let legacy = serde_json::from_slice::<LegacyCacheIndex>(bytes).ok();
    let empty = serde_json::to_vec(&CacheIndex {
        schema: INDEX_SCHEMA,
        entries: Vec::new(),
    })
    .map_err(|error| CacheError::Index(error.to_string()))?;
    write_atomic(&root.join(INDEX_FILE), &empty)?;
    if let Some(legacy) = legacy.filter(|legacy| legacy.schema == 1) {
        for entry in legacy.entries.into_iter().take(MAX_ENTRIES) {
            if entry.tile.validate().is_ok()
                && is_sha256_hex(&entry.sha256)
                && entry.byte_len > 0
                && entry.byte_len <= MAX_TILE_BYTES as u64
            {
                if let Ok(path) = tile_path(root, &entry.tile, &entry.sha256) {
                    quarantine_then_remove(&path);
                }
            }
        }
    }
    Ok(Vec::new())
}

fn recover_corrupt_regular_index(root: &Path) -> Result<Vec<CacheEntry>, CacheError> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let path = root.join(INDEX_FILE);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| CacheError::Io(error.to_string()))?;
    if !metadata.file_type().is_file() {
        return Err(CacheError::Index(
            "corrupt index is not a regular file".to_string(),
        ));
    }

    let quarantine = root.join(format!(
        ".corrupt-index-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::rename(&path, &quarantine).map_err(|error| CacheError::Io(error.to_string()))?;
    let empty = serde_json::to_vec(&CacheIndex {
        schema: INDEX_SCHEMA,
        entries: Vec::new(),
    })
    .map_err(|error| CacheError::Index(error.to_string()))?;
    if let Err(error) = write_atomic(&path, &empty) {
        let _ = std::fs::rename(&quarantine, &path);
        return Err(error);
    }
    let _ = std::fs::remove_file(quarantine);
    Ok(Vec::new())
}

fn ensure_root(root: &Path) -> Result<(), CacheError> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(CacheError::Policy("cache root must be a real directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root).map_err(|error| CacheError::Io(error.to_string()))?;
            let metadata = std::fs::symlink_metadata(root)
                .map_err(|error| CacheError::Io(error.to_string()))?;
            if metadata.file_type().is_dir() {
                Ok(())
            } else {
                Err(CacheError::Policy("cache root must be a real directory"))
            }
        }
        Err(error) => Err(CacheError::Io(error.to_string())),
    }
}

fn ensure_tile_parent(root: &Path, tile: &TileId) -> Result<(), CacheError> {
    let components = [
        tile.region.as_str().to_string(),
        tile.z.to_string(),
        tile.x.to_string(),
    ];
    let mut current = root.to_path_buf();
    for component in components {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(CacheError::Policy(
                    "tile parent must not be a symlink or file",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|error| CacheError::Io(error.to_string()))?;
            }
            Err(error) => return Err(CacheError::Io(error.to_string())),
        }
    }
    Ok(())
}

fn tile_path(root: &Path, tile: &TileId, digest: &str) -> Result<PathBuf, CacheError> {
    tile.validate()
        .map_err(|_| CacheError::Policy("tile identity is invalid"))?;
    if !is_sha256_hex(digest) {
        return Err(CacheError::Digest);
    }
    Ok(root
        .join(tile.region.as_str())
        .join(tile.z.to_string())
        .join(tile.x.to_string())
        .join(format!("{}-{digest}.tile", tile.y)))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or(CacheError::Policy("cache path has no parent"))?;
    let temporary = parent.join(format!(
        ".cache-{}-{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(|error| CacheError::Io(error.to_string()))
}

fn quarantine_then_remove(path: &Path) {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let quarantine = path.with_file_name(format!(
        ".corrupt-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::rename(path, &quarantine).is_ok() {
        let _ = std::fs::remove_file(quarantine);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline_catalog::{sha256_hex, RegionId};

    fn catalog() -> VerifiedCatalog {
        let bytes = br#"{"schema":1,"provider":"openstreetmap-derived","regions":[{"region_id":"test-region","revision":"r1","min_zoom":0,"max_zoom":18,"expires_at_ms":999999}]}"#;
        VerifiedCatalog::admit_json(bytes, &sha256_hex(bytes)).unwrap()
    }

    fn tile(x: u32) -> TileId {
        TileId::new(RegionId::parse("test-region").unwrap(), 2, x, 1).unwrap()
    }

    #[test]
    fn quota_is_never_exceeded_and_eviction_is_lru() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache =
            OfflineTileCache::open(dir.path(), CachePolicy::bounded(6, 10_000).unwrap()).unwrap();
        let catalog = catalog();
        for (id, now) in [(tile(0), 1), (tile(1), 2)] {
            cache
                .store_verified(&catalog, id, b"abc", &sha256_hex(b"abc"), now)
                .unwrap();
        }
        assert!(matches!(
            cache.lookup(&catalog, &tile(0), 3),
            OfflineTile::Verified { .. }
        ));
        cache
            .store_verified(&catalog, tile(2), b"xyz", &sha256_hex(b"xyz"), 4)
            .unwrap();
        assert!(cache.used_bytes() <= 6);
        assert!(matches!(
            cache.lookup(&catalog, &tile(1), 5),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));
        assert!(matches!(
            cache.lookup(&catalog, &tile(0), 5),
            OfflineTile::Verified { .. }
        ));
    }

    #[test]
    fn quota_property_holds_across_varying_tiles_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CachePolicy::bounded(32, 10_000).unwrap();
        let catalog = catalog();
        let region = RegionId::parse("test-region").unwrap();
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        for index in 0_u32..64 {
            let id = TileId::new(region.clone(), 8, index, 1).unwrap();
            let bytes = vec![index as u8; (index as usize % 7) + 1];
            cache
                .store_verified(
                    &catalog,
                    id,
                    &bytes,
                    &sha256_hex(&bytes),
                    u64::from(index) + 1,
                )
                .unwrap();
            assert!(cache.used_bytes() <= policy.quota_bytes);
            cache = OfflineTileCache::open(dir.path(), policy).unwrap();
            assert!(cache.used_bytes() <= policy.quota_bytes);
        }
    }

    #[test]
    fn offline_lookup_returns_only_digest_verified_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CachePolicy::bounded(1024, 100).unwrap();
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        let id = tile(0);
        cache
            .store_verified(
                &catalog(),
                id.clone(),
                b"verified",
                &sha256_hex(b"verified"),
                10,
            )
            .unwrap();
        assert_eq!(
            cache.lookup(&catalog(), &id, 11),
            OfflineTile::Verified {
                bytes: b"verified".to_vec(),
                sha256: sha256_hex(b"verified")
            }
        );
        assert!(matches!(
            cache.lookup(&catalog(), &tile(1), 11),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));
        assert!(matches!(
            cache.lookup(&catalog(), &id, 111),
            OfflineTile::Unavailable(UnavailableReason::Expired)
        ));
    }

    #[test]
    fn future_cache_timestamps_are_rejected_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CachePolicy::bounded(1024, 10_000).unwrap();
        let id = tile(0);
        let digest = sha256_hex(b"future");
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        cache
            .store_verified(&catalog(), id.clone(), b"future", &digest, 10)
            .unwrap();

        let index_path = dir.path().join(INDEX_FILE);
        let mut index: CacheIndex = serde_json::from_slice(&std::fs::read(&index_path).unwrap())
            .unwrap();
        index.entries[0].verified_at_ms = 20;
        index.entries[0].last_access_ms = 20;
        std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();

        let mut restarted = OfflineTileCache::open(dir.path(), policy).unwrap();
        assert!(matches!(
            restarted.lookup(&catalog(), &id, 19),
            OfflineTile::Unavailable(UnavailableReason::CorruptRemoved)
        ));
        assert!(matches!(
            restarted.lookup(&catalog(), &id, 19),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));
    }

    #[test]
    fn corruption_is_quarantined_removed_and_does_not_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CachePolicy::bounded(1024, 1000).unwrap();
        let id = tile(0);
        let digest = sha256_hex(b"good");
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        cache
            .store_verified(&catalog(), id.clone(), b"good", &digest, 1)
            .unwrap();
        std::fs::write(tile_path(dir.path(), &id, &digest).unwrap(), b"evil").unwrap();
        assert!(matches!(
            cache.lookup(&catalog(), &id, 2),
            OfflineTile::Unavailable(UnavailableReason::CorruptRemoved)
        ));
        assert!(!tile_path(dir.path(), &id, &digest).unwrap().exists());
        let mut restarted = OfflineTileCache::open(dir.path(), policy).unwrap();
        assert!(matches!(
            restarted.lookup(&catalog(), &id, 3),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_tile_parent_is_rejected_without_escape() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), dir.path().join("test-region")).unwrap();
        let mut cache =
            OfflineTileCache::open(dir.path(), CachePolicy::bounded(1024, 1000).unwrap()).unwrap();
        let result = cache.store_verified(&catalog(), tile(0), b"good", &sha256_hex(b"good"), 1);
        assert!(matches!(result, Err(CacheError::Policy(_))));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn catalog_replacement_or_expiry_revokes_cached_tile() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CachePolicy::bounded(1024, 10_000).unwrap();
        let id = tile(0);
        let bytes = b"verified";
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        let admitted = catalog();
        cache
            .store_verified(&admitted, id.clone(), bytes, &sha256_hex(bytes), 1)
            .unwrap();
        cache = OfflineTileCache::open(dir.path(), policy).unwrap();

        let replacement_bytes = br#"{"schema":1,"provider":"openstreetmap-derived","regions":[{"region_id":"test-region","revision":"r2","min_zoom":0,"max_zoom":18,"expires_at_ms":999999}]}"#;
        let replacement =
            VerifiedCatalog::admit_json(replacement_bytes, &sha256_hex(replacement_bytes)).unwrap();
        assert!(matches!(
            cache.lookup(&replacement, &id, 2),
            OfflineTile::Unavailable(UnavailableReason::CatalogRejectedRemoved)
        ));
        assert!(matches!(
            cache.lookup(&replacement, &id, 2),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));

        cache
            .store_verified(&admitted, id.clone(), bytes, &sha256_hex(bytes), 3)
            .unwrap();
        assert!(matches!(
            cache.lookup(&admitted, &id, 1_000_000),
            OfflineTile::Unavailable(UnavailableReason::CatalogRejectedRemoved)
        ));
    }

    #[test]
    fn schema_one_cache_is_invalidated_without_bricking_startup() {
        let dir = tempfile::tempdir().unwrap();
        let id = tile(0);
        let legacy_bytes = b"legacy";
        let legacy_digest = sha256_hex(legacy_bytes);
        ensure_tile_parent(dir.path(), &id).unwrap();
        let legacy_path = tile_path(dir.path(), &id, &legacy_digest).unwrap();
        std::fs::write(&legacy_path, legacy_bytes).unwrap();
        let legacy_index = serde_json::json!({
            "schema": 1,
            "entries": [{
                "tile": id,
                "sha256": legacy_digest,
                "byte_len": legacy_bytes.len(),
                "verified_at_ms": 1,
                "last_access_ms": 1
            }]
        });
        std::fs::write(
            dir.path().join(INDEX_FILE),
            serde_json::to_vec(&legacy_index).unwrap(),
        )
        .unwrap();

        let policy = CachePolicy::bounded(4, 10_000).unwrap();
        let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
        assert!(cache.is_empty());
        assert!(!legacy_path.exists(), "unbound legacy payload survived");
        let migrated: CacheIndex =
            serde_json::from_slice(&std::fs::read(dir.path().join(INDEX_FILE)).unwrap()).unwrap();
        assert_eq!(migrated.schema, INDEX_SCHEMA);
        assert!(migrated.entries.is_empty());
        assert!(matches!(
            cache.lookup(&catalog(), &tile(0), 2),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));

        let fresh = b"new";
        cache
            .store_verified(&catalog(), tile(0), fresh, &sha256_hex(fresh), 3)
            .unwrap();
        assert!(matches!(
            cache.lookup(&catalog(), &tile(0), 4),
            OfflineTile::Verified { bytes, .. } if bytes == fresh
        ));

        let malformed_dir = tempfile::tempdir().unwrap();
        let malformed_id = tile(1);
        ensure_tile_parent(malformed_dir.path(), &malformed_id).unwrap();
        let isolated_path =
            tile_path(malformed_dir.path(), &malformed_id, &sha256_hex(b"unbound")).unwrap();
        std::fs::write(&isolated_path, b"unbound").unwrap();
        std::fs::write(
            malformed_dir.path().join(INDEX_FILE),
            br#"{"schema":1,"entries":[{"malformed":true}]}"#,
        )
        .unwrap();
        let mut malformed = OfflineTileCache::open(malformed_dir.path(), policy).unwrap();
        assert!(malformed.is_empty());
        assert!(
            isolated_path.exists(),
            "unparseable payload should remain isolated, not be followed blindly"
        );
        assert!(matches!(
            malformed.lookup(&catalog(), &malformed_id, 2),
            OfflineTile::Unavailable(UnavailableReason::NotIndexed)
        ));
    }

    #[test]
    fn corrupt_current_index_recovers_empty_without_admitting_hostile_metadata() {
        let hostile_indexes: [&[u8]; 2] = [
            br#"{"schema":2,"entries":["#,
            br#"{"schema":2,"entries":[{"tile":null}]}"#,
        ];
        let policy = CachePolicy::bounded(1024, 10_000).unwrap();

        for hostile in hostile_indexes {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(INDEX_FILE), hostile).unwrap();

            let mut cache = OfflineTileCache::open(dir.path(), policy).unwrap();
            assert!(cache.is_empty());
            let recovered: CacheIndex =
                serde_json::from_slice(&std::fs::read(dir.path().join(INDEX_FILE)).unwrap())
                    .unwrap();
            assert_eq!(recovered.schema, INDEX_SCHEMA);
            assert!(recovered.entries.is_empty());

            let bytes = b"recovered";
            cache
                .store_verified(&catalog(), tile(0), bytes, &sha256_hex(bytes), 1)
                .unwrap();
            assert!(matches!(
                cache.lookup(&catalog(), &tile(0), 2),
                OfflineTile::Verified { bytes: found, .. } if found == bytes
            ));
        }

        let future_dir = tempfile::tempdir().unwrap();
        let future = br#"{"schema":65535,"entries":[]}"#;
        std::fs::write(future_dir.path().join(INDEX_FILE), future).unwrap();
        assert!(matches!(
            OfflineTileCache::open(future_dir.path(), policy),
            Err(CacheError::Index(error)) if error == "index schema is unsupported"
        ));
        assert_eq!(
            std::fs::read(future_dir.path().join(INDEX_FILE)).unwrap(),
            future,
            "a future-generation index must not be destroyed by an older reader"
        );
    }
}
