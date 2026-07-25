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
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use mde_collab_types::value::sha256_hex;
use mde_collab_types::PayloadRef;

use crate::error::{CollabError, Result};

const SHA256_HEX_LEN: usize = 64;
/// Payloads are materialized in memory by this store. Keep the limit aligned
/// with the Communications file/clipboard transfer ceiling.
const MAX_BLOB_BYTES: u64 = 100 * 1024 * 1024;

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

    fn path_for(&self, digest: &str) -> Option<PathBuf> {
        if !is_canonical_digest(digest) {
            return None;
        }
        let shard = &digest[..2];
        Some(self.root.join(shard).join(digest))
    }

    /// Return only an existing regular, non-symlink blob path. A digest can be
    /// received from a signed-but-hostile peer, and the store must not let it
    /// turn `contains`/`purge` into arbitrary path operations.
    fn existing_blob_path(&self, digest: &str) -> Option<PathBuf> {
        let path = self.path_for(digest)?;
        let shard = path.parent()?;
        let root_meta = fs::symlink_metadata(&self.root).ok()?;
        let shard_meta = fs::symlink_metadata(shard).ok()?;
        let blob_meta = fs::symlink_metadata(&path).ok()?;
        if root_meta.file_type().is_symlink()
            || !root_meta.file_type().is_dir()
            || shard_meta.file_type().is_symlink()
            || !shard_meta.file_type().is_dir()
            || blob_meta.file_type().is_symlink()
            || !blob_meta.file_type().is_file()
        {
            return None;
        }
        Some(path)
    }

    fn reject_unsafe_parent(&self, path: &Path) -> std::io::Result<()> {
        for directory in [self.root.as_path(), path.parent().unwrap_or(&self.root)] {
            if let Ok(metadata) = fs::symlink_metadata(directory) {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "blob store path is not a regular directory: {}",
                            directory.display()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn open_blob_no_follow(path: &Path) -> std::io::Result<File> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;

            // Linux fcntl.h: O_NOFOLLOW == 00400000 (octal).
            return OpenOptions::new()
                .read(true)
                .custom_flags(0o400000)
                .open(path);
        }

        #[cfg(not(target_os = "linux"))]
        {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("refusing symlinked blob {}", path.display()),
                ));
            }
            File::open(path)
        }
    }
}

fn is_canonical_digest(digest: &str) -> bool {
    digest.len() == SHA256_HEX_LEN
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn oversized_blob_error(len: u64) -> CollabError {
    CollabError::Io(format!(
        "blob is {len} bytes, exceeding the {MAX_BLOB_BYTES}-byte limit"
    ))
}

impl BlobStore for FsBlobStore {
    fn put(&mut self, bytes: &[u8]) -> Result<PayloadRef> {
        if bytes.len() as u64 > MAX_BLOB_BYTES {
            return Err(oversized_blob_error(bytes.len() as u64));
        }
        let reference = PayloadRef::of_bytes(bytes);
        let path = self
            .path_for(&reference.sha256_hex)
            .expect("PayloadRef::of_bytes always creates a canonical digest");
        self.reject_unsafe_parent(&path)?;
        if self.existing_blob_path(&reference.sha256_hex).is_some() {
            return Ok(reference);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.reject_unsafe_parent(&path)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("blob path is not a regular file: {}", path.display()),
                )
                .into());
            }
            return Ok(reference);
        }
        // Write to a temp sibling then rename, so a reader never sees a partial
        // blob under its final content-addressed name.
        let tmp = path.with_extension("tmp");
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)?;
        Ok(reference)
    }

    fn get(&self, reference: &PayloadRef) -> Result<Vec<u8>> {
        if reference.len > MAX_BLOB_BYTES {
            return Err(oversized_blob_error(reference.len));
        }
        let Some(path) = self.existing_blob_path(&reference.sha256_hex) else {
            return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
        };
        let file = match Self::open_blob_no_follow(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
        }
        if metadata.len() > MAX_BLOB_BYTES {
            return Err(oversized_blob_error(metadata.len()));
        }
        if metadata.len() != reference.len {
            return Err(CollabError::BlobSizeMismatch {
                expected: reference.len,
                actual: metadata.len(),
            });
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_BLOB_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BLOB_BYTES {
            return Err(oversized_blob_error(bytes.len() as u64));
        }
        verify_bytes(&bytes, reference)?;
        Ok(bytes)
    }

    fn contains(&self, sha256_hex: &str) -> bool {
        self.existing_blob_path(sha256_hex).is_some()
    }

    fn purge(&mut self, sha256_hex: &str) -> Result<bool> {
        let Some(path) = self.existing_blob_path(sha256_hex) else {
            return Ok(false);
        };
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

#[cfg(test)]
mod tests {
    use super::{is_canonical_digest, BlobStore, FsBlobStore, MAX_BLOB_BYTES};
    use mde_collab_types::value::sha256_hex;
    use mde_collab_types::PayloadRef;

    #[test]
    fn canonical_digest_validation_rejects_path_components() {
        assert!(is_canonical_digest(&"a".repeat(64)));
        assert!(!is_canonical_digest(&"A".repeat(64)));
        assert!(!is_canonical_digest("../outside"));
        assert!(!is_canonical_digest(&"f".repeat(65)));
    }

    #[test]
    fn fs_store_rejects_malformed_digest_without_path_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir
            .path()
            .parent()
            .expect("tempdir parent")
            .join("mde-collab-blob-escape-target");
        std::fs::write(&outside, b"must survive").expect("write sentinel");
        let mut store = FsBlobStore::new(dir.path());
        let reference = PayloadRef {
            sha256_hex: "../mde-collab-blob-escape-target".into(),
            len: 12,
            content_type: None,
        };

        assert!(!store.contains(&reference.sha256_hex));
        assert!(!store.purge(&reference.sha256_hex).expect("purge probe"));
        assert!(matches!(
            store.get(&reference),
            Err(crate::error::CollabError::BlobNotFound(_))
        ));
        assert_eq!(
            std::fs::read(&outside).expect("sentinel remains"),
            b"must survive"
        );
        std::fs::remove_file(outside).expect("remove sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn fs_store_treats_final_symlink_as_absent_for_all_reads_and_purge() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root tempdir");
        let target_dir = tempfile::tempdir().expect("target tempdir");
        let target = target_dir.path().join("outside");
        std::fs::write(&target, b"outside bytes").expect("write target");
        let store = FsBlobStore::new(root.path());
        let reference = PayloadRef::of_bytes(b"outside bytes");
        let path = store
            .path_for(&reference.sha256_hex)
            .expect("canonical path");
        std::fs::create_dir_all(path.parent().expect("shard")).expect("create shard");
        symlink(&target, &path).expect("create final symlink");

        assert!(!store.contains(&reference.sha256_hex));
        assert!(matches!(
            store.get(&reference),
            Err(crate::error::CollabError::BlobNotFound(_))
        ));
        let mut store = store;
        assert!(!store.purge(&reference.sha256_hex).expect("purge probe"));
        assert!(path.is_symlink());
        assert_eq!(
            std::fs::read(&target).expect("target remains"),
            b"outside bytes"
        );
    }

    #[test]
    fn fs_store_rejects_oversized_files_before_materializing_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let digest = sha256_hex(b"oversized");
        let reference = PayloadRef {
            sha256_hex: digest.clone(),
            len: MAX_BLOB_BYTES + 1,
            content_type: None,
        };
        let path = store.path_for(&digest).expect("canonical path");
        std::fs::create_dir_all(path.parent().expect("shard")).expect("create shard");
        let file = std::fs::File::create(&path).expect("create sparse blob");
        file.set_len(MAX_BLOB_BYTES + 1).expect("grow sparse blob");

        let error = store.get(&reference).expect_err("oversized blob");
        assert!(
            matches!(error, crate::error::CollabError::Io(message) if message.contains("exceeding"))
        );
    }
}
