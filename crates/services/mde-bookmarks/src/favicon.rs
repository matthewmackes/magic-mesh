//! The content-addressed, deduped favicon store (lock Q6).
//!
//! Favicons live *outside* the bookmark tree in a `hash -> bytes` map. A
//! bookmark only references a [`ContentHash`]; identical icons across many
//! bookmarks store exactly one blob. The store is a CRDT-friendly grow-only map
//! (content-addressed, so two nodes that hash the same bytes agree on the key,
//! and [`FaviconStore::merge`] is a plain union). Pruning unreferenced blobs is
//! the worker's job (BOOKMARKS-2) — this crate keeps it pure.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::ContentHash;

/// Hash raw icon bytes into their content address (SHA-256).
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    ContentHash(out)
}

/// A grow-only `hash -> bytes` favicon store (lock Q6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaviconStore {
    blobs: BTreeMap<String, Vec<u8>>,
}

impl FaviconStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert icon bytes, returning their content hash. A second insert of the
    /// same bytes is a no-op that returns the same hash (dedup).
    pub fn insert(&mut self, bytes: Vec<u8>) -> ContentHash {
        let hash = hash_bytes(&bytes);
        self.blobs.entry(hash.to_hex()).or_insert(bytes);
        hash
    }

    /// Fetch the bytes for a hash, if present.
    #[must_use]
    pub fn get(&self, hash: &ContentHash) -> Option<&[u8]> {
        self.blobs.get(&hash.to_hex()).map(Vec::as_slice)
    }

    /// Whether the store holds a blob for this hash.
    #[must_use]
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.blobs.contains_key(&hash.to_hex())
    }

    /// The number of distinct blobs held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }

    /// Merge another store in (grow-only union). Content addressing makes this
    /// conflict-free: an identical hash carries identical bytes, so it does not
    /// matter which side is kept.
    pub fn merge(&mut self, other: &Self) {
        for (k, v) in &other.blobs {
            self.blobs.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_dedup_to_one_blob() {
        let mut store = FaviconStore::new();
        let h1 = store.insert(b"icon-bytes".to_vec());
        let h2 = store.insert(b"icon-bytes".to_vec());
        assert_eq!(h1, h2, "same bytes -> same content hash");
        assert_eq!(store.len(), 1, "deduped to one blob");
        assert_eq!(store.get(&h1), Some(&b"icon-bytes"[..]));
    }

    #[test]
    fn distinct_bytes_are_distinct_blobs() {
        let mut store = FaviconStore::new();
        let a = store.insert(b"aaa".to_vec());
        let b = store.insert(b"bbb".to_vec());
        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
        assert!(store.contains(&a) && store.contains(&b));
    }

    #[test]
    fn merge_is_a_conflict_free_union() {
        let mut left = FaviconStore::new();
        let a = left.insert(b"a".to_vec());
        let mut right = FaviconStore::new();
        let b = right.insert(b"b".to_vec());
        left.merge(&right);
        assert!(left.contains(&a) && left.contains(&b));
        assert_eq!(left.len(), 2);
    }
}
