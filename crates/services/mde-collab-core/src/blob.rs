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
use std::sync::atomic::{AtomicU64, Ordering};

use mde_collab_types::value::sha256_hex;
use mde_collab_types::PayloadRef;
use sha2::{Digest, Sha256};

use crate::error::{CollabError, Result};

const SHA256_HEX_LEN: usize = 64;
/// Payloads are materialized in memory by this store. Keep the limit aligned
/// with the Communications file/clipboard transfer ceiling.
const MAX_BLOB_BYTES: u64 = 100 * 1024 * 1024;
static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// A private, verified blob which has not yet been installed in canonical CAS.
///
/// Dropping or aborting this value removes only its create-new staging inode.
/// Call [`commit`](Self::commit) immediately before the durable operation which
/// will reference the blob.
#[derive(Debug)]
pub struct FsBlobStage {
    path: Option<PathBuf>,
    file: File,
    canonical_path: PathBuf,
    reference: PayloadRef,
}

/// An installed blob whose canonical inode is still owned by this transaction.
///
/// [`retain`](Self::retain) makes the installation permanent after the caller's
/// durable reference commit succeeds. Until then, abort/Drop removes the
/// canonical path only when it still names the exact inode installed by this
/// token. A concurrent replacement is never removed.
#[derive(Debug)]
pub struct FsBlobCommit {
    canonical_path: PathBuf,
    file: Option<File>,
    reference: PayloadRef,
    owns_install: bool,
}

impl FsBlobStage {
    /// The exact digest and length verified while streaming into this stage.
    #[must_use]
    pub const fn reference(&self) -> &PayloadRef {
        &self.reference
    }

    /// Atomically install this stage without replacing an existing CAS entry.
    ///
    /// A hard link is the portable no-replace primitive here: the staging file
    /// and canonical file are siblings on one filesystem, and `hard_link`
    /// fails with `AlreadyExists` instead of overwriting a concurrent writer.
    ///
    /// # Errors
    ///
    /// Returns an error when the stage cannot be linked, verified, sealed, or
    /// synchronized.
    ///
    /// # Panics
    ///
    /// Panics only if the internally validated staging path is unexpectedly
    /// absent.
    pub fn commit(mut self) -> Result<FsBlobCommit> {
        let commit_file = self.file.try_clone()?;
        let path = self.path.as_ref().expect("live staging path");
        let owns_install = match fs::hard_link(path, &self.canonical_path) {
            Ok(()) => {
                let canonical = match FsBlobStore::open_blob_no_follow(&self.canonical_path) {
                    Ok(file) => file,
                    Err(error) => {
                        remove_if_same_file(&self.canonical_path, &self.file);
                        return Err(error.into());
                    }
                };
                let bound = match same_file(&canonical, &self.file) {
                    Ok(bound) => bound,
                    Err(error) => {
                        remove_if_same_file(&self.canonical_path, &self.file);
                        return Err(error.into());
                    }
                };
                if !bound {
                    remove_if_same_file(&self.canonical_path, &self.file);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "canonical blob did not bind to the owned staging inode",
                    )
                    .into());
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = verify_file_at_path(&self.canonical_path, &self.reference)?;
                seal_file_read_only(&existing)?;
                let current = FsBlobStore::open_blob_no_follow(&self.canonical_path)?;
                if !same_file(&existing, &current)? {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "canonical blob changed while sealing replay authority",
                    )
                    .into());
                }
                false
            }
            Err(error) => return Err(error.into()),
        };

        if let Some(path) = self.path.clone() {
            match fs::remove_file(&path) {
                Ok(()) => {
                    self.path = None;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.path = None;
                }
                Err(error) => {
                    if owns_install {
                        remove_if_same_file(&self.canonical_path, &self.file);
                    }
                    return Err(error.into());
                }
            }
        }
        if let Err(error) = sync_parent(&self.canonical_path) {
            if owns_install {
                remove_if_same_file(&self.canonical_path, &self.file);
            }
            return Err(error.into());
        }

        Ok(FsBlobCommit {
            canonical_path: self.canonical_path.clone(),
            file: Some(commit_file),
            reference: self.reference.clone(),
            owns_install,
        })
    }

    /// Explicitly discard this private stage. Drop provides the same cleanup.
    ///
    /// # Errors
    ///
    /// Returns an error when the private staging file cannot be removed.
    pub fn abort(mut self) -> Result<()> {
        self.remove_stage()
    }

    fn remove_stage(&mut self) -> Result<()> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        match fs::remove_file(&path) {
            Ok(()) => {
                sync_parent(&path)?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for FsBlobStage {
    fn drop(&mut self) {
        let _ = self.remove_stage();
    }
}

impl FsBlobCommit {
    /// The exact digest and length installed or found in canonical CAS.
    #[must_use]
    pub const fn reference(&self) -> &PayloadRef {
        &self.reference
    }

    /// Whether this token, rather than a concurrent idempotent writer,
    /// installed the canonical inode.
    #[must_use]
    pub const fn owns_install(&self) -> bool {
        self.owns_install
    }

    /// Keep the canonical blob after the caller durably commits its reference.
    #[must_use]
    pub fn retain(mut self) -> PayloadRef {
        self.owns_install = false;
        self.file = None;
        self.reference.clone()
    }

    /// Roll back this token's canonical installation, if it is still the same
    /// inode. An idempotent token which found another writer's blob is a no-op.
    ///
    /// # Errors
    ///
    /// Returns an error when a still-owned canonical file cannot be removed or
    /// its directory cannot be synchronized.
    pub fn abort(mut self) -> Result<()> {
        self.cleanup_install()
    }

    fn cleanup_install(&mut self) -> Result<()> {
        if !self.owns_install {
            return Ok(());
        }
        self.owns_install = false;
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        if remove_if_same_file(&self.canonical_path, &file) {
            sync_parent(&self.canonical_path)?;
        }
        Ok(())
    }
}

impl Drop for FsBlobCommit {
    fn drop(&mut self) {
        let _ = self.cleanup_install();
    }
}

/// Verify that `bytes` match `reference` (both digest and length). The single
/// integrity gate every fetch funnels through.
///
/// # Errors
///
/// Returns a size- or hash-mismatch error when `bytes` do not match
/// `reference`.
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
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot materialize the blob.
    fn put(&mut self, bytes: &[u8]) -> Result<PayloadRef>;

    /// Fetch and **verify** the bytes for `reference`. Errors with
    /// [`CollabError::BlobNotFound`] if absent, or a hash/size-mismatch error if
    /// the stored bytes do not match the reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is absent, unreadable, or fails integrity
    /// verification.
    fn get(&self, reference: &PayloadRef) -> Result<Vec<u8>>;

    /// Whether a blob with this lower-hex SHA-256 digest is present (no verify).
    fn contains(&self, sha256_hex: &str) -> bool;

    /// Remove the blob with this digest. Returns `true` if it existed. Callers
    /// gate this on the tombstone purge rule (all known members acked) — the
    /// store itself imposes no policy.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing blob cannot be safely verified or
    /// removed.
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

/// A filesystem content-addressed store under a root.
///
/// It uses `<root>/<ab>/<digest>`, sharded by the first digest byte to keep
/// directories shallow. The real
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
            OpenOptions::new()
                .read(true)
                .custom_flags(0o400_000)
                .open(path)
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

    /// Stream a blob into a private create-new file while enforcing the exact
    /// caller-provided length and SHA-256. No unverified bytes become visible
    /// at the canonical content address.
    ///
    /// # Errors
    ///
    /// Returns an error when the expected reference is invalid, the stage cannot
    /// be created or synchronized, or the reader's bytes do not match it.
    ///
    /// # Panics
    ///
    /// Panics only if an internally constructed canonical path has no parent.
    pub fn stage<R: Read>(&self, mut reader: R, expected: &PayloadRef) -> Result<FsBlobStage> {
        if expected.len > MAX_BLOB_BYTES {
            return Err(oversized_blob_error(expected.len));
        }
        if !is_canonical_digest(&expected.sha256_hex) {
            return Err(CollabError::BlobHashMismatch {
                expected: expected.sha256_hex.clone(),
                actual: "non-canonical expected SHA-256".to_owned(),
            });
        }

        let canonical_path = self
            .path_for(&expected.sha256_hex)
            .expect("validated canonical digest");
        self.reject_unsafe_parent(&canonical_path)?;
        let parent = canonical_path
            .parent()
            .expect("CAS path has shard")
            .to_owned();
        fs::create_dir_all(&parent)?;
        self.reject_unsafe_parent(&canonical_path)?;

        let (path, file) = create_private_stage(&parent, &expected.sha256_hex)?;
        let mut stage = FsBlobStage {
            path: Some(path),
            file,
            canonical_path,
            reference: expected.clone(),
        };

        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let remaining = expected.len.saturating_add(1).saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let read_len = usize::try_from(remaining.min(buffer.len() as u64))
                .expect("bounded read length fits usize");
            let count = reader.read(&mut buffer[..read_len])?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > expected.len {
                return Err(CollabError::BlobSizeMismatch {
                    expected: expected.len,
                    actual: total,
                });
            }
            hasher.update(&buffer[..count]);
            stage.file.write_all(&buffer[..count])?;
        }

        if total != expected.len {
            return Err(CollabError::BlobSizeMismatch {
                expected: expected.len,
                actual: total,
            });
        }
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected.sha256_hex {
            return Err(CollabError::BlobHashMismatch {
                expected: expected.sha256_hex.clone(),
                actual,
            });
        }
        stage.file.sync_all()?;
        seal_file_read_only(&stage.file)?;
        sync_directory(&parent)?;
        Ok(stage)
    }
}

fn create_private_stage(parent: &Path, digest: &str) -> std::io::Result<(PathBuf, File)> {
    for _ in 0..128 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{digest}.{}.{}.tmp", std::process::id(), id));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private blob staging file",
    ))
}

fn verify_file_at_path(path: &Path, reference: &PayloadRef) -> Result<File> {
    let mut file = FsBlobStore::open_blob_no_follow(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(CollabError::BlobNotFound(reference.sha256_hex.clone()));
    }
    if metadata.len() != reference.len {
        return Err(CollabError::BlobSizeMismatch {
            expected: reference.len,
            actual: metadata.len(),
        });
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        CollabError::Io("blob is too large to materialize on this platform".to_owned())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(MAX_BLOB_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    verify_bytes(&bytes, reference)?;
    Ok(file)
}

fn verify_open_file_digest(file: &mut File, expected_digest: &str) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(CollabError::BlobNotFound(expected_digest.to_owned()));
    }
    if metadata.len() > MAX_BLOB_BYTES {
        return Err(oversized_blob_error(metadata.len()));
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_digest {
        return Err(CollabError::BlobHashMismatch {
            expected: expected_digest.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn seal_file_read_only(file: &File) -> std::io::Result<()> {
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions)?;
    file.sync_all()
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(unix)]
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_file(_left: &File, _right: &File) -> std::io::Result<bool> {
    // std does not expose a stable file identity on these targets. Refusing to
    // claim ownership is safer than unlinking a same-length replacement.
    Ok(false)
}

fn remove_if_same_file(path: &Path, owned: &File) -> bool {
    let Ok(current) = FsBlobStore::open_blob_no_follow(path) else {
        return false;
    };
    if !same_file(&current, owned).unwrap_or(false) {
        return false;
    }
    fs::remove_file(path).is_ok()
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
        let reference = PayloadRef::of_bytes(bytes);
        let stage = self.stage(std::io::Cursor::new(bytes), &reference)?;
        Ok(stage.commit()?.retain())
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
        let capacity = usize::try_from(metadata.len()).map_err(|_| {
            CollabError::Io("blob is too large to materialize on this platform".to_owned())
        })?;
        let mut bytes = Vec::with_capacity(capacity);
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
        let mut owned = match Self::open_blob_no_follow(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        verify_open_file_digest(&mut owned, sha256_hex)?;
        if !remove_if_same_file(&path, &owned) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical blob changed after purge identity verification",
            )
            .into());
        }
        sync_parent(&path)?;
        Ok(true)
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
    use std::io::{Cursor, Read};
    use std::sync::{Arc, Barrier};

    fn staging_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let Ok(shards) = std::fs::read_dir(root) else {
            return found;
        };
        for shard in shards.flatten() {
            let Ok(entries) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            found.extend(entries.flatten().filter_map(|entry| {
                let name = entry.file_name();
                name.to_string_lossy()
                    .ends_with(".tmp")
                    .then(|| entry.path())
            }));
        }
        found
    }

    struct FailingReader {
        bytes: Cursor<Vec<u8>>,
        successful_reads_left: usize,
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.successful_reads_left == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "injected stream failure",
                ));
            }
            self.successful_reads_left -= 1;
            let limit = buffer.len().min(3);
            self.bytes.read(&mut buffer[..limit])
        }
    }

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

    #[test]
    fn fs_put_uses_verified_staging_without_tmp_residue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = b"production put through owned staging";
        let mut store = FsBlobStore::new(dir.path());

        let reference = store.put(bytes).expect("put");

        assert_eq!(store.get(&reference).expect("get"), bytes);
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn fs_stage_rejects_hash_and_length_mismatch_without_tmp_residue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let expected = PayloadRef::of_bytes(b"expected");

        assert!(matches!(
            store.stage(Cursor::new(b"attacker"), &expected),
            Err(crate::error::CollabError::BlobHashMismatch { .. })
        ));
        assert!(staging_files(dir.path()).is_empty());

        assert!(matches!(
            store.stage(Cursor::new(b"expected-extra"), &expected),
            Err(crate::error::CollabError::BlobSizeMismatch { .. })
        ));
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn fs_stage_stream_failure_and_drop_clean_private_inode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let expected = PayloadRef::of_bytes(b"streamed bytes");
        let reader = FailingReader {
            bytes: Cursor::new(b"streamed bytes".to_vec()),
            successful_reads_left: 1,
        };

        assert!(matches!(
            store.stage(reader, &expected),
            Err(crate::error::CollabError::Io(message)) if message.contains("injected")
        ));
        assert!(staging_files(dir.path()).is_empty());

        let stage = store
            .stage(Cursor::new(b"streamed bytes"), &expected)
            .expect("stage");
        assert_eq!(staging_files(dir.path()).len(), 1);
        drop(stage);
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn concurrent_commits_never_replace_and_only_installer_can_abort() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let bytes = b"concurrent identical content";
        let expected = PayloadRef::of_bytes(bytes);
        let first = store
            .stage(Cursor::new(bytes), &expected)
            .expect("first stage");
        let second = store
            .stage(Cursor::new(bytes), &expected)
            .expect("second stage");
        let barrier = Arc::new(Barrier::new(3));

        let first_barrier = Arc::clone(&barrier);
        let first_thread = std::thread::spawn(move || {
            first_barrier.wait();
            first.commit().expect("first commit")
        });
        let second_barrier = Arc::clone(&barrier);
        let second_thread = std::thread::spawn(move || {
            second_barrier.wait();
            second.commit().expect("second commit")
        });
        barrier.wait();

        let mut commits = vec![
            first_thread.join().expect("first thread"),
            second_thread.join().expect("second thread"),
        ];
        assert_eq!(
            commits
                .iter()
                .filter(|commit| commit.owns_install())
                .count(),
            1
        );
        let non_owner_index = commits
            .iter()
            .position(|commit| !commit.owns_install())
            .expect("non-owner");
        let non_owner = commits.remove(non_owner_index);
        non_owner.abort().expect("non-owner abort");
        assert_eq!(store.get(&expected).expect("winner remains"), bytes);
        let _ = commits.pop().expect("owner").retain();
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn owned_commit_abort_removes_only_its_canonical_inode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let bytes = b"transaction candidate";
        let expected = PayloadRef::of_bytes(bytes);
        let commit = store
            .stage(Cursor::new(bytes), &expected)
            .expect("stage")
            .commit()
            .expect("commit");
        assert!(commit.owns_install());

        commit.abort().expect("abort");

        assert!(!store.contains(&expected.sha256_hex));
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn retained_blob_replay_cannot_leave_canonical_bytes_owner_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let bytes = b"immutable authenticated collaboration payload";
        let expected = PayloadRef::of_bytes(bytes);
        let _ = store
            .stage(Cursor::new(bytes), &expected)
            .expect("first stage")
            .commit()
            .expect("first commit")
            .retain();
        let replay = store
            .stage(Cursor::new(bytes), &expected)
            .expect("replay stage")
            .commit()
            .expect("idempotent replay");
        assert!(!replay.owns_install());
        let _ = replay.retain();

        let canonical = store.path_for(&expected.sha256_hex).expect("path");
        assert!(std::fs::metadata(&canonical)
            .expect("canonical metadata")
            .permissions()
            .readonly());
        let mutation = std::fs::OpenOptions::new().write(true).open(&canonical);
        assert!(matches!(
            mutation,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(store.get(&expected).expect("immutable bytes"), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn abort_does_not_remove_a_replacement_canonical_inode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsBlobStore::new(dir.path());
        let bytes = b"owned bytes";
        let expected = PayloadRef::of_bytes(bytes);
        let commit = store
            .stage(Cursor::new(bytes), &expected)
            .expect("stage")
            .commit()
            .expect("commit");
        let canonical = store.path_for(&expected.sha256_hex).expect("path");
        std::fs::remove_file(&canonical).expect("remove owned canonical");
        std::fs::write(&canonical, b"another writer's inode").expect("replacement");

        commit.abort().expect("guarded abort");

        assert_eq!(
            std::fs::read(&canonical).expect("replacement remains"),
            b"another writer's inode"
        );
        assert!(staging_files(dir.path()).is_empty());
    }

    #[test]
    fn restarted_purge_cannot_unlink_a_concurrent_non_cas_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = b"durable CAS bytes";
        let mut writer = FsBlobStore::new(dir.path());
        let reference = writer.put(bytes).expect("initial put");
        let canonical = writer.path_for(&reference.sha256_hex).expect("path");
        drop(writer);

        let mut restarted = FsBlobStore::new(dir.path());
        std::fs::remove_file(&canonical).expect("retire original inode");
        std::fs::write(&canonical, b"concurrent non-CAS replacement")
            .expect("install hostile replacement");

        assert!(matches!(
            restarted.purge(&reference.sha256_hex),
            Err(crate::error::CollabError::BlobHashMismatch { .. })
        ));
        assert_eq!(
            std::fs::read(&canonical).expect("replacement survives"),
            b"concurrent non-CAS replacement"
        );
    }
}
