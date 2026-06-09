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
    let meta = std::fs::metadata(path)
        .map_err(|e| CaError::Io(format!("metadata {}: {e}", path.display())))?;
    enforce_seal(path, &meta)?;
    std::fs::read(path).map_err(|e| CaError::Io(format!("read {}: {e}", path.display())))
}

/// Write a sealed file with mode 0600. Creates the parent
/// directory if missing. Atomic-ish via write-then-chmod
/// (Linux's open(O_CREAT, mode) honours umask; we re-chmod
/// to guarantee the exact bits the seal expects).
///
/// # Errors
///
///   * [`CaError::Io`] when mkdir / write / chmod fails.
pub fn write_sealed(path: &Path, bytes: &[u8]) -> Result<(), CaError> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CaError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(path, bytes)
        .map_err(|e| CaError::Io(format!("write {}: {e}", path.display())))?;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| CaError::Io(format!("metadata {}: {e}", path.display())))?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
        .map_err(|e| CaError::Io(format!("chmod {}: {e}", path.display())))
}

/// Pure permission check — given a Metadata, verifies the
/// file is sealed (mode 0600 + owner matches current uid).
fn enforce_seal(path: &Path, meta: &std::fs::Metadata) -> Result<(), CaError> {
    use std::os::unix::fs::MetadataExt;
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
}
