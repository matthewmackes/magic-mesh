//! NF-2.4 (v2.5) — sealed-file helpers.
//!
//! The Nebula CA private key MUST be readable only by the
//! running process owner with mode 0600 and owner = current
//! uid. These helpers enforce those invariants on every read
//! so a group/world-readable key (operator typo, broken
//! backup-restore, etc.) errors loudly instead of silently
//! exposing the CA private material.

use std::path::Path;

use super::CaError;

/// Maximum sealed material read into memory from a local filesystem boundary.
/// Nebula keys, certificates, CSRs, and authority pins are all far smaller;
/// this protects callers from a hostile or corrupted replacement file before
/// allocation or PEM/JSON materialization.
pub const MAX_SEALED_FILE_BYTES: u64 = 1024 * 1024;

/// Open a regular file without following a symlink in the final path
/// component. Nonblocking open prevents a hostile FIFO from stalling an
/// authority read. Callers read from the returned descriptor so the inode
/// checked is the inode consumed, closing the metadata/read replacement
/// window.
fn open_read_no_follow(path: &Path) -> Result<std::fs::File, CaError> {
    use rustix::fs::{Mode, OFlags};

    reject_symlinked_parents(path)?;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| CaError::Io(format!("open {}: {e}", path.display())))?;
    Ok(fd.into())
}

/// Open a sealed file exposed through the Nebula identity `current` switch.
/// The switch is intentionally a symlink, but only to one owner-controlled,
/// mode-0700 generation directory beneath the same identity root is allowed.
fn open_generation_read(path: &Path) -> Result<Option<std::fs::File>, CaError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Component;

    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let parent_meta = std::fs::symlink_metadata(parent)
        .map_err(|e| CaError::Io(format!("inspect {}: {e}", parent.display())))?;
    if !parent_meta.file_type().is_symlink() {
        return Ok(None);
    }
    if parent.file_name() != Some(std::ffi::OsStr::new("current")) {
        return Err(CaError::Io(format!(
            "refusing symlinked parent {}",
            parent.display()
        )));
    }
    let identity_root = parent
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no identity root", path.display())))?;
    reject_symlinked_parents(&identity_root.join(".sealed-leaf"))?;
    let identity_meta = std::fs::symlink_metadata(identity_root).map_err(|e| {
        CaError::Io(format!(
            "inspect identity root {}: {e}",
            identity_root.display()
        ))
    })?;
    if !identity_meta.is_dir()
        || identity_meta.uid() != rustix::process::getuid().as_raw()
        || identity_meta.permissions().mode() & 0o777 != 0o700
    {
        return Err(CaError::Io(format!(
            "unsafe identity root {}",
            identity_root.display()
        )));
    }
    let target = std::fs::read_link(parent)
        .map_err(|e| CaError::Io(format!("read identity switch {}: {e}", parent.display())))?;
    let target_components: Vec<_> = target.components().collect();
    let [Component::Normal(generation_name)] = target_components.as_slice() else {
        return Err(CaError::Io(format!(
            "refusing untrusted identity switch {}",
            parent.display()
        )));
    };
    let generation_dir = identity_root.join(generation_name);
    let generation_meta = std::fs::symlink_metadata(&generation_dir).map_err(|e| {
        CaError::Io(format!(
            "inspect identity generation {}: {e}",
            generation_dir.display()
        ))
    })?;
    if generation_meta.file_type().is_symlink()
        || !generation_meta.is_dir()
        || generation_meta.uid() != rustix::process::getuid().as_raw()
        || generation_meta.permissions().mode() & 0o777 != 0o700
    {
        return Err(CaError::Io(format!(
            "unsafe identity generation {}",
            generation_dir.display()
        )));
    }
    let leaf = path
        .file_name()
        .ok_or_else(|| CaError::Io(format!("{} has no filename", path.display())))?;
    let resolved = generation_dir.join(leaf);
    let fd = rustix::fs::open(
        &resolved,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| CaError::Io(format!("open {}: {e}", path.display())))?;
    Ok(Some(fd.into()))
}

/// Consume a descriptor through the shared sealed-file byte ceiling. The
/// descriptor is already opened with the required no-follow policy, so the
/// metadata and bounded read apply to the inode that will actually be used.
fn read_bounded_file(file: std::fs::File, path: &Path) -> Result<Vec<u8>, CaError> {
    use std::io::Read as _;

    let metadata = file
        .metadata()
        .map_err(|e| CaError::Io(format!("metadata {}: {e}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(CaError::Io(format!(
            "read {}: not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_SEALED_FILE_BYTES {
        return Err(CaError::Io(format!(
            "read {}: exceeds {MAX_SEALED_FILE_BYTES}-byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SEALED_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CaError::Io(format!("read {}: {e}", path.display())))?;
    if bytes.len() as u64 > MAX_SEALED_FILE_BYTES {
        return Err(CaError::Io(format!(
            "read {}: exceeds {MAX_SEALED_FILE_BYTES}-byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Read a non-secret file without following an untrusted symlink at its leaf.
///
/// Canonical CA-pair and Nebula-identity generation links are accepted because
/// [`open_sealed_read`] validates their complete owner-controlled shape. Other
/// attacker-controlled links remain refused; this reader intentionally does
/// not apply the private-key mode-0600 check.
pub fn read_no_follow(path: &Path) -> Result<Vec<u8>, CaError> {
    read_bounded_file(open_sealed_read(path)?, path)
}

/// Read a sealed file, enforcing mode-0600 + owner-matches-
/// running-uid invariants. Returns the file content as bytes
/// on success.
///
/// # Errors
///
///   * [`CaError::Io`] when the file is missing or read
///     fails.
///   * [`CaError::InsecurePermissions`] when:
///     - any bit outside the owner-rw triplet is set (group
///       or world permissions present); OR
///     - the file's owner uid doesn't match the running
///       process's effective uid.
pub fn read_sealed(path: &Path) -> Result<Vec<u8>, CaError> {
    let file = open_sealed_read(path)?;
    let meta = file
        .metadata()
        .map_err(|e| CaError::Io(format!("metadata {}: {e}", path.display())))?;
    enforce_seal(path, &meta)?;
    read_bounded_file(file, path)
}

/// Open a sealed file directly, allowing only the generation-link shape owned
/// by [`write_atomic_pair`]. A canonical `ca.key`/`ca.crt` view is a symlink by
/// design; arbitrary symlinks remain refused. The pair and generation
/// directories are checked before the final no-follow open, and every link
/// target is a single relative name, so the resolver cannot leave the pair
/// root or traverse `..`.
fn open_sealed_read(path: &Path) -> Result<std::fs::File, CaError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| CaError::Io(format!("inspect {}: {e}", path.display())))?;
    if !metadata.file_type().is_symlink() {
        if let Some(file) = open_generation_read(path)? {
            return Ok(file);
        }
        return open_read_no_follow(path);
    }
    // Canonical pair views are symlinks at the leaf, but their containing
    // directory must still be resolved without following a hostile parent.
    reject_symlinked_parents(path)?;

    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Component;

    let target = std::fs::read_link(path)
        .map_err(|e| CaError::Io(format!("read sealed link {}: {e}", path.display())))?;
    let components: Vec<_> = target.components().collect();
    let [Component::Normal(pair_name), Component::Normal(current_name), Component::Normal(leaf)] =
        components.as_slice()
    else {
        return Err(CaError::Io(format!(
            "refusing untrusted sealed link {}",
            path.display()
        )));
    };
    let expected_leaf = path
        .file_name()
        .ok_or_else(|| CaError::Io(format!("{} has no filename", path.display())))?;
    if *leaf != expected_leaf
        || !pair_name.to_string_lossy().starts_with('.')
        || !pair_name.to_string_lossy().ends_with(".pair")
        || *current_name != std::ffi::OsStr::new("current")
    {
        return Err(CaError::Io(format!(
            "refusing untrusted sealed link {}",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent", path.display())))?;
    let pair_root = parent.join(pair_name);
    let pair_meta = std::fs::symlink_metadata(&pair_root)
        .map_err(|e| CaError::Io(format!("inspect sealed pair {}: {e}", pair_root.display())))?;
    if pair_meta.file_type().is_symlink()
        || !pair_meta.is_dir()
        || pair_meta.uid() != rustix::process::getuid().as_raw()
        || pair_meta.permissions().mode() & 0o777 != 0o700
    {
        return Err(CaError::Io(format!(
            "unsafe sealed pair directory {}",
            pair_root.display()
        )));
    }
    let current = pair_root.join("current");
    let generation = std::fs::read_link(&current)
        .map_err(|e| CaError::Io(format!("read sealed generation {}: {e}", current.display())))?;
    let generation_components: Vec<_> = generation.components().collect();
    let [Component::Normal(generation_name)] = generation_components.as_slice() else {
        return Err(CaError::Io(format!(
            "refusing untrusted sealed generation {}",
            current.display()
        )));
    };
    let generation_dir = pair_root.join(generation_name);
    let generation_meta = std::fs::symlink_metadata(&generation_dir).map_err(|e| {
        CaError::Io(format!(
            "inspect sealed generation {}: {e}",
            generation_dir.display()
        ))
    })?;
    if generation_meta.file_type().is_symlink()
        || !generation_meta.is_dir()
        || generation_meta.uid() != rustix::process::getuid().as_raw()
        || generation_meta.permissions().mode() & 0o777 != 0o700
    {
        return Err(CaError::Io(format!(
            "unsafe sealed generation directory {}",
            generation_dir.display()
        )));
    }
    // Open the generation that was validated above, rather than resolving the
    // mutable `current` switch a second time after the check; this closes the
    // switch-replacement window for the descriptor that is actually consumed.
    let resolved = generation_dir.join(leaf);
    let fd = rustix::fs::open(
        &resolved,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| CaError::Io(format!("open {}: {e}", path.display())))?;
    Ok(fd.into())
}

/// Write a sealed file with mode 0600. Creates the parent
/// directory if missing. The file is opened with mode 0600 from its first
/// inode-visible instant, then re-chmodded to repair an existing path.
///
/// # Errors
///
///   * [`CaError::Io`] when mkdir / write / chmod fails.
pub fn write_sealed(path: &Path, bytes: &[u8]) -> Result<(), CaError> {
    write_atomic_sealed(path, bytes)
}

/// Create a sealed file without replacing an existing leaf.
///
/// The staged inode is written and synced before an atomic same-directory hard
/// link creates `path`. A hard link (rather than `rename`) is intentional: a
/// concurrent writer cannot replace an authority pin after this function has
/// observed it as absent. Returns `true` when this call created the file and
/// `false` when the destination already existed. The caller must inspect an
/// existing file before treating it as equivalent.
pub fn write_sealed_if_absent(path: &Path, bytes: &[u8]) -> Result<bool, CaError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent directory", path.display())))?;
    reject_symlinked_parents(path)?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    reject_symlinked_parents(path)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CaError::Io(format!("invalid output filename {}", path.display())))?;
    let (tmp, mut file) = (0..16)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".{leaf}.create.{}.{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(CaError::Io(format!(
                    "create temp {}: {error}",
                    candidate.display()
                )))),
            }
        })
        .unwrap_or_else(|| {
            Err(CaError::Io(format!(
                "tempfile collisions for {}",
                path.display()
            )))
        })?;

    let result = (|| {
        file.write_all(bytes)
            .map_err(|e| CaError::Io(format!("write temp {}: {e}", tmp.display())))?;
        file.sync_all()
            .map_err(|e| CaError::Io(format!("fsync temp {}: {e}", tmp.display())))?;
        drop(file);
        match std::fs::hard_link(&tmp, path) {
            Ok(()) => {
                std::fs::remove_file(&tmp).map_err(|e| {
                    CaError::Io(format!("remove staged sealed file {}: {e}", tmp.display()))
                })?;
                let parent_handle = std::fs::File::open(parent)
                    .map_err(|e| CaError::Io(format!("open parent {}: {e}", parent.display())))?;
                parent_handle
                    .sync_all()
                    .map_err(|e| CaError::Io(format!("fsync parent {}: {e}", parent.display())))?;
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(CaError::Io(format!(
                "create sealed file {}: {error}",
                path.display()
            ))),
        }
    })();
    if result.as_ref().map_or(true, |created| !created) {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Crash-durable same-directory replacement at mode 0600. A unique
/// `create_new` tempfile avoids symlink-following and writer collisions; after
/// the file is synced and renamed, the parent directory is synced so the rename
/// itself survives power loss.
pub fn write_atomic_sealed(path: &Path, bytes: &[u8]) -> Result<(), CaError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent directory", path.display())))?;
    reject_symlinked_parents(path)?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    reject_symlinked_parents(path)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CaError::Io(format!("invalid output filename {}", path.display())))?;
    let (tmp, mut file) = (0..16)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".{leaf}.tmp.{}.{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(CaError::Io(format!(
                    "create temp {}: {error}",
                    candidate.display()
                )))),
            }
        })
        .unwrap_or_else(|| {
            Err(CaError::Io(format!(
                "tempfile collisions for {}",
                path.display()
            )))
        })?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|e| CaError::Io(format!("write temp {}: {e}", tmp.display())))?;
        file.sync_all()
            .map_err(|e| CaError::Io(format!("fsync temp {}: {e}", tmp.display())))?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| {
            CaError::Io(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        let parent_handle = std::fs::File::open(parent)
            .map_err(|e| CaError::Io(format!("open parent {}: {e}", parent.display())))?;
        parent_handle
            .sync_all()
            .map_err(|e| CaError::Io(format!("fsync parent {}: {e}", parent.display())))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Refuse to resolve a CA/sealed-file path through a symlinked directory. The
/// final file itself is intentionally handled by no-follow-open or atomic
/// rename; only its parent components are checked here because `create_dir_all`
/// otherwise follows a replicated directory link before those protections run.
fn reject_symlinked_parents(path: &Path) -> Result<(), CaError> {
    let parent = path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent directory", path.display())))?;
    let mut current = std::path::PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CaError::Io(format!(
                    "refusing symlinked parent {}",
                    current.display()
                )));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(CaError::Io(format!(
                    "sealed-file parent is not a directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CaError::Io(format!(
                    "inspect sealed-file parent {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

/// Install a public certificate and its private key behind one atomic
/// generation switch. Once the two canonical symlinks have been initialized,
/// readers can observe only the complete old generation or the complete new
/// generation—never a certificate from one with a key from the other.
///
/// Existing regular-file pairs are deliberately not migrated in place because
/// replacing two independent names cannot be atomic. Operators must perform
/// that one-time migration while the consuming service is stopped.
pub fn write_atomic_pair(
    public_path: &Path,
    public_bytes: &[u8],
    secret_path: &Path,
    secret_bytes: &[u8],
) -> Result<(), CaError> {
    use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt, PermissionsExt};

    let parent = public_path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent", public_path.display())))?;
    if secret_path.parent() != Some(parent) {
        return Err(CaError::Io(
            "atomic certificate/key pair paths must share one directory".into(),
        ));
    }
    reject_symlinked_parents(public_path)?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    reject_symlinked_parents(public_path)?;
    let public_leaf = public_path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| CaError::Io(format!("invalid filename {}", public_path.display())))?;
    let secret_leaf = secret_path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| CaError::Io(format!("invalid filename {}", secret_path.display())))?;
    let pair_root = parent.join(format!(".{public_leaf}-{secret_leaf}.pair"));

    match std::fs::symlink_metadata(&pair_root) {
        Ok(meta) => {
            if !meta.file_type().is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != rustix::process::getuid().as_raw()
                || meta.permissions().mode() & 0o777 != 0o700
            {
                return Err(CaError::Io(format!(
                    "unsafe atomic-pair directory {}",
                    pair_root.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&pair_root).map_err(|e| {
                CaError::Io(format!(
                    "create pair directory {}: {e}",
                    pair_root.display()
                ))
            })?;
        }
        Err(error) => {
            return Err(CaError::Io(format!(
                "inspect pair directory {}: {error}",
                pair_root.display()
            )));
        }
    }

    let expected_public_target = Path::new(&format!(".{public_leaf}-{secret_leaf}.pair"))
        .join("current")
        .join(public_leaf);
    let expected_secret_target = Path::new(&format!(".{public_leaf}-{secret_leaf}.pair"))
        .join("current")
        .join(secret_leaf);
    let public_exists = std::fs::symlink_metadata(public_path).is_ok();
    let secret_exists = std::fs::symlink_metadata(secret_path).is_ok();
    if public_exists != secret_exists {
        return Err(CaError::Io(format!(
            "refusing incomplete atomic pair {} + {}",
            public_path.display(),
            secret_path.display()
        )));
    }
    if public_exists {
        let public_target = std::fs::read_link(public_path).map_err(|_| {
            CaError::Io(format!(
                "refusing to replace legacy regular-file pair {} + {} while live",
                public_path.display(),
                secret_path.display()
            ))
        })?;
        let secret_target = std::fs::read_link(secret_path).map_err(|_| {
            CaError::Io(format!(
                "refusing to replace legacy regular-file pair {} + {} while live",
                public_path.display(),
                secret_path.display()
            ))
        })?;
        if public_target != expected_public_target || secret_target != expected_secret_target {
            return Err(CaError::Io(
                "canonical atomic-pair symlink target mismatch".into(),
            ));
        }
    }

    let generation = (0..16)
        .find_map(|_| {
            let candidate = pair_root.join(format!(
                "generation-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(CaError::Io(format!(
                    "create pair generation {}: {error}",
                    candidate.display()
                )))),
            }
        })
        .unwrap_or_else(|| Err(CaError::Io("atomic-pair generation collisions".into())))?;
    let stage = (|| {
        write_atomic_sealed(&generation.join(public_leaf), public_bytes)?;
        write_atomic_sealed(&generation.join(secret_leaf), secret_bytes)?;
        std::fs::File::open(&generation)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| CaError::Io(format!("fsync generation {}: {e}", generation.display())))
    })();
    if let Err(error) = stage {
        let _ = std::fs::remove_dir_all(&generation);
        return Err(error);
    }

    let generation_leaf = generation
        .file_name()
        .ok_or_else(|| CaError::Io("pair generation has no leaf".into()))?;
    let switch = pair_root.join(format!(
        ".current.tmp.{}.{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    symlink(generation_leaf, &switch)
        .map_err(|e| CaError::Io(format!("create pair switch {}: {e}", switch.display())))?;

    if !public_exists {
        // Initialize the stable canonical views before exposing the generation.
        // Both links are dangling until `current` is renamed below.
        install_pair_symlink(public_path, &expected_public_target)?;
        if let Err(error) = install_pair_symlink(secret_path, &expected_secret_target) {
            let _ = std::fs::remove_file(public_path);
            let _ = std::fs::remove_file(&switch);
            let _ = std::fs::remove_dir_all(&generation);
            return Err(error);
        }
    }
    if let Err(error) = std::fs::rename(&switch, pair_root.join("current")) {
        let _ = std::fs::remove_file(&switch);
        if !public_exists {
            let _ = std::fs::remove_file(public_path);
            let _ = std::fs::remove_file(secret_path);
        }
        let _ = std::fs::remove_dir_all(&generation);
        return Err(CaError::Io(format!(
            "activate certificate/key pair: {error}"
        )));
    }
    std::fs::File::open(&pair_root)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| CaError::Io(format!("fsync pair root {}: {e}", pair_root.display())))?;
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| CaError::Io(format!("fsync pair parent {}: {e}", parent.display())))
}

fn install_pair_symlink(path: &Path, target: &Path) -> Result<(), CaError> {
    use std::os::unix::fs::symlink;

    let parent = path
        .parent()
        .ok_or_else(|| CaError::Io(format!("{} has no parent", path.display())))?;
    let temp = parent.join(format!(
        ".pair-link.tmp.{}.{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    symlink(target, &temp)
        .map_err(|e| CaError::Io(format!("create pair symlink {}: {e}", temp.display())))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        CaError::Io(format!("install pair symlink {}: {e}", path.display()))
    })
}

/// Pure permission check — given a Metadata, verifies the
/// file is sealed (mode 0600 + owner matches current uid).
fn enforce_seal(path: &Path, meta: &std::fs::Metadata) -> Result<(), CaError> {
    use std::os::unix::fs::MetadataExt;
    if !meta.file_type().is_file() {
        return Err(CaError::Io(format!(
            "read {}: not a regular file",
            path.display()
        )));
    }
    let mode = meta.mode() & 0o777;
    // Allow exactly 0o600 — reject anything else (including
    // 0o400 which would lock writes; the CA workflow needs
    // both directions).
    if mode != 0o600 {
        return Err(CaError::InsecurePermissions {
            path: path.display().to_string(),
            mode,
        });
    }
    let uid_running = rustix::process::getuid().as_raw();
    if meta.uid() != uid_running {
        // We surface the same error variant so log scraping
        // doesn't need a parallel path — the message field
        // contains the path, which is enough for the
        // operator to chown manually.
        return Err(CaError::InsecurePermissions {
            path: path.display().to_string(),
            mode,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn write_then_read_round_trips_at_mode_0600() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("ca.key");
        let payload = b"-----BEGIN NEBULA CA KEY-----\nbody\n-----END NEBULA CA KEY-----\n";
        write_sealed(&path, payload).expect("write");
        // Confirm the bits landed at 0600.
        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
        // Read back via the sealed path — must succeed
        // because we own the file (we just created it) +
        // the bits are right.
        let got = read_sealed(&path).expect("read");
        assert_eq!(got, payload);
    }

    #[test]
    fn read_rejects_world_readable_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("world-readable.key");
        std::fs::write(&path, b"unsafe").expect("seed");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644); // world-readable
        std::fs::set_permissions(&path, perms).unwrap();

        let err = read_sealed(&path).unwrap_err();
        match err {
            CaError::InsecurePermissions { mode, .. } => {
                assert_eq!(mode & 0o777, 0o644);
            }
            other => panic!("expected InsecurePermissions, got {other:?}"),
        }
    }

    #[test]
    fn read_rejects_group_readable_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("group-readable.key");
        std::fs::write(&path, b"unsafe").expect("seed");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o640);
        std::fs::set_permissions(&path, perms).unwrap();

        let err = read_sealed(&path).unwrap_err();
        assert!(matches!(err, CaError::InsecurePermissions { .. }));
    }

    #[test]
    fn read_missing_file_returns_io() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("never-existed.key");
        let err = read_sealed(&path).unwrap_err();
        assert!(matches!(err, CaError::Io(_)));
    }

    #[test]
    fn write_creates_missing_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("deep/nested/dir/ca.key");
        write_sealed(&path, b"body").expect("write");
        assert!(path.exists());
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn atomic_write_replaces_a_hostile_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let victim = tmp.path().join("victim");
        let output = tmp.path().join("ca.key");
        std::fs::write(&victim, b"do-not-touch").expect("victim");
        symlink(&victim, &output).expect("hostile symlink");
        write_atomic_sealed(&output, b"sealed").expect("replace safely");
        assert_eq!(std::fs::read(&victim).unwrap(), b"do-not-touch");
        assert_eq!(std::fs::read(&output).unwrap(), b"sealed");
        assert!(!std::fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp.")));
    }

    #[test]
    fn read_rejects_a_hostile_symlink_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let victim = tmp.path().join("victim");
        let link = tmp.path().join("ca.crt");
        std::fs::write(&victim, b"victim").expect("victim");
        symlink(&victim, &link).expect("hostile symlink");

        let error = read_no_follow(&link).expect_err("symlink must be refused");
        assert!(error.to_string().contains("ca.crt"));
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
    }

    #[cfg(unix)]
    #[test]
    fn sealed_file_helpers_reject_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let parent = tmp.path().join("ca");
        symlink(outside.path(), &parent).expect("hostile parent");
        let path = parent.join("ca.key");

        assert!(write_atomic_sealed(&path, b"must-not-write").is_err());
        assert!(read_no_follow(&path).is_err());
        assert!(!outside.path().join("ca.key").exists());
    }

    #[test]
    fn read_sealed_rejects_a_symlink_even_when_target_is_sealed() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("target.key");
        let link = tmp.path().join("ca.key");
        write_sealed(&target, b"secret").expect("sealed target");
        symlink(&target, &link).expect("hostile symlink");

        assert!(
            read_sealed(&link).is_err(),
            "sealed reads must not follow links"
        );
    }

    #[test]
    fn reads_reject_non_regular_authority_inputs() {
        let tmp = tempfile::tempdir().expect("tempdir");

        assert!(
            read_no_follow(tmp.path()).is_err(),
            "directories are not inputs"
        );
        assert!(
            read_sealed(tmp.path()).is_err(),
            "directories are not secrets"
        );
    }

    #[test]
    fn reads_reject_oversized_sealed_material_before_allocation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("oversized.key");
        write_sealed(&path, &vec![b'x'; (MAX_SEALED_FILE_BYTES + 1) as usize])
            .expect("write oversized fixture");

        let no_follow = read_no_follow(&path).expect_err("oversized public read must fail closed");
        assert!(no_follow.to_string().contains("exceeds"));
        let sealed = read_sealed(&path).expect_err("oversized sealed read must fail closed");
        assert!(sealed.to_string().contains("exceeds"));
    }

    #[test]
    fn atomic_pair_switch_never_exposes_mixed_generations() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cert = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        write_atomic_pair(&cert, b"CERT-A", &key, b"KEY-A").expect("first generation");
        write_atomic_pair(&cert, b"CERT-B", &key, b"KEY-B").expect("second generation");
        assert_eq!(std::fs::read(&cert).unwrap(), b"CERT-B");
        assert_eq!(read_sealed(&key).unwrap(), b"KEY-B");
        assert!(std::fs::symlink_metadata(&cert)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::symlink_metadata(&key)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn atomic_pair_refuses_partial_or_legacy_live_pair() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cert = tmp.path().join("ca.crt");
        let key = tmp.path().join("ca.key");
        std::fs::write(&cert, b"legacy-cert").expect("legacy cert");
        let error = write_atomic_pair(&cert, b"new-cert", &key, b"new-key")
            .expect_err("partial pair must fail closed");
        assert!(error.to_string().contains("incomplete atomic pair"));
        assert_eq!(std::fs::read(&cert).unwrap(), b"legacy-cert");
        assert!(!key.exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_pair_rejects_a_symlinked_parent_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let parent = tmp.path().join("ca");
        symlink(outside.path(), &parent).expect("hostile parent");
        let cert = parent.join("ca.crt");
        let key = parent.join("ca.key");

        let error = write_atomic_pair(&cert, b"cert", &key, b"key")
            .expect_err("pair writes must reject symlinked parents");
        assert!(error.to_string().contains("refusing symlinked parent"));
        assert!(!outside.path().join("ca.crt").exists());
        assert!(!outside.path().join("ca.key").exists());
        assert!(!outside.path().join(".ca.crt-ca.key.pair").exists());
    }

    #[cfg(unix)]
    #[test]
    fn canonical_pair_reads_reject_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let real_cert = outside.path().join("ca.crt");
        let real_key = outside.path().join("ca.key");
        write_atomic_pair(&real_cert, b"cert", &real_key, b"key").expect("pair");

        let alias = tmp.path().join("alias");
        symlink(outside.path(), &alias).expect("hostile parent");
        let cert = alias.join("ca.crt");
        let key = alias.join("ca.key");

        assert!(read_no_follow(&cert).is_err());
        assert!(read_sealed(&key).is_err());
    }
}
