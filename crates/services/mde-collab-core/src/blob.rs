//! The content-addressed blob store.
//!
//! Large substance — a document snapshot, a CRDT update, a file's bytes — never
//! rides inside a signed envelope; the envelope carries a small [`PayloadRef`]
//! (the SHA-256 digest + length), and the bytes live here, addressed by digest
//! under the per-user MDE data root. Fetching **always verifies** the bytes hash
//! and length against the reference before returning them, so a corrupt or
//! substituted blob can never reach projection or the surface.
//!
//! The trait keeps the boundary injectable: [`FsBlobStore`] is the real
//! per-user store; [`MemoryBlobStore`] backs tests.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_collab_types::value::sha256_hex;
use mde_collab_types::PayloadRef;

use crate::error::{CollabError, Result};

/// Verify that `bytes` match `reference` (both digest and length). The single
/// integrity gate every fetch funnels through.
pub fn verify_bytes(bytes: &[u8], reference: &PayloadRef) -> Result<()> {
    let actual_len = bytes.len() as u64;
    if actual_len != reference.len {
        return Err(CollabError::BlobSizeMismatch {
            expected: reference.len,
            actual: actual_len,
        });
    }
    let actual = sha256_hex(bytes);
    if actual != reference.sha256_hex {
        return Err(CollabError::BlobHashMismatch {
            expected: reference.sha256_hex.clone(),
            actual,
        });
    }
    Ok(())
}

/// A store of payloads keyed by the SHA-256 of their bytes.
pub trait BlobStore {
    /// Store `bytes`, returning the content-addressed [`PayloadRef`] (digest +
    /// length) the caller then puts on an event. Storing the same bytes twice is
    /// idempotent (same digest, same location).
    fn put(&mut self, bytes: &[u8]) -> Result<PayloadRef>;

    /// Fetch and **verify** the bytes for `reference`. Errors with
    /// [`CollabError::BlobNotFound`] if absent, or a hash/size-mismatch error if
    /// the stored bytes do not match the reference.
    fn get(&self, reference: &PayloadRef) -> Result<Vec<u8>>;

    /// Whether a blob with this lower-hex SHA-256 digest is present (no verify).
    fn contains(&self, sha256_hex: &str) -> bool;

    /// Remove the blob with this digest. Returns `true` if it existed. Callers
    /// gate this on the tombstone purge rule (all known members acked) — the
    /// store itself imposes no policy.
    fn purge(&mut self, sha256_hex: &str) -> Result<bool>;
}

/// An in-memory blob store (tests, transient staging).
#[derive(Debug, Default, Clone)]
pub struct MemoryBlobStore {
    blobs: HashMap<String, Vec<u8>>,
}

impl MemoryBlobStore {
    /// A fresh empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&mut self, bytes: &[u8]) -> Result<PayloadRef> {
        let reference = PayloadRef::of_bytes(bytes);
        self.blobs
            .entry(reference.sha256_hex.clone())
            .or_insert_with(|| bytes.to_vec());
        Ok(reference)
    }

    fn get(&self, reference: &PayloadRef) -> Result<Vec<u8>> {
        let bytes = self
            .blobs
            .get(&reference.sha256_hex)
            .ok_or_else(|| CollabError::BlobNotFound(reference.sha256_hex.clone()))?;
        verify_bytes(bytes, reference)?;
        Ok(bytes.clone())
    }

    fn contains(&self, sha256_hex: &str) -> bool {
        self.blobs.contains_key(sha256_hex)
    }

    fn purge(&mut self, sha256_hex: &str) -> Result<bool> {
        Ok(self.blobs.remove(sha256_hex).is_some())
    }
}

/// A filesystem content-addressed store under a root (`<root>/<ab>/<digest>`,
/// sharded by the first digest byte to keep directories shallow). The real
/// per-user store; see [`default_root`] for the MDE data-root default.
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// A store rooted at `root` (created on first `put`).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default per-user blob root: `<data_dir>/mde/collab/blobs`. `None` when
    /// no data dir is resolvable (a headless context with no `$HOME`/XDG) — the
    /// caller then injects an explicit root.
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("mde").join("collab").join("blobs"))
    }

    fn path_for(&self, digest: &str) -> PathBuf {
        let shard = digest.get(0..2).unwrap_or("00");
        self.root.join(shard).join(digest)
    }
}

impl BlobStore for FsBlobStore {
    fn put(&mut self, bytes: &[u8]) -> Result<PayloadRef> {
        let reference = PayloadRef::of_bytes(bytes);
        let path = self.path_for(&reference.sha256_hex);
        if path.exists() {
            return Ok(reference);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write to a temp sibling then rename, so a reader never sees a partial
        // blob under its final content-addressed name.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(reference)
    }

    fn get(&self, reference: &PayloadRef) -> Result<Vec<u8>> {
        let path = self.path_for(&reference.sha256_hex);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        verify_bytes(&bytes, reference)?;
        Ok(bytes)
    }

    fn contains(&self, sha256_hex: &str) -> bool {
        self.path_for(sha256_hex).exists()
    }

    fn purge(&mut self, sha256_hex: &str) -> Result<bool> {
        let path = self.path_for(sha256_hex);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// The default per-user MDE data root for the collaboration core
/// (`<data_dir>/mde/collab`), or `None` if no data dir resolves. The actor logs
/// live under `<root>/logs`, the blobs under `<root>/blobs`.
#[must_use]
pub fn default_root() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("mde").join("collab"))
}
