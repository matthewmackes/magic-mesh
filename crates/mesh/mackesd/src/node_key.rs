//! SEC-6 — the persisted node signing key.
//!
//! A stable Ed25519 keypair per box (distinct from the per-enroll
//! `EnrolledIdentity`, which is minted fresh each enrollment, and
//! from the Nebula cert key, which stays single-purpose per §3).
//! Used to sign gossiped retract records so peers can attribute and
//! tamper-check revocations (Q28/29). Created on first use, sealed
//! at 0600.

use std::io::{self, Read};
use std::path::Path;

use ed25519_dalek::SigningKey;

/// Default on-disk location.
pub const DEFAULT_KEY_PATH: &str = "/var/lib/mackesd/node-signing.key";

const NODE_SEED_BYTES: usize = 32;

/// Read an existing signing seed through a bounded descriptor boundary.
///
/// The seed is a security-sensitive replicated/runtime identity. Do not follow
/// a final symlink or open a special file that could block the daemon, and do
/// not let a growing or oversized leaf reach the key parser.
fn read_existing_seed(path: &Path) -> io::Result<[u8; NODE_SEED_BYTES]> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000 | 0o4000 | 0o2000000); // O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "node signing seed is not a regular file",
            ));
        }
    }

    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "node signing seed is not a regular file",
        ));
    }

    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "node signing seed is not a regular file",
        ));
    }
    if metadata.len() > NODE_SEED_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is larger than a {}-byte ed25519 seed",
                path.display(),
                NODE_SEED_BYTES
            ),
        ));
    }

    let mut bytes = Vec::with_capacity(NODE_SEED_BYTES + 1);
    file.take((NODE_SEED_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() != NODE_SEED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a {}-byte ed25519 seed",
                path.display(),
                NODE_SEED_BYTES
            ),
        ));
    }

    let mut seed = [0_u8; NODE_SEED_BYTES];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

/// Load the node signing key, creating it on first use.
///
/// # Errors
/// IO failures (unreadable dir, bad permissions).
pub fn load_or_create(path: &Path) -> io::Result<SigningKey> {
    match read_existing_seed(path) {
        Ok(seed) => Ok(SigningKey::from_bytes(&seed)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            use rand::RngCore;
            use std::io::Write as _;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut seed = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)?;
            let mut permissions = file.metadata()?.permissions();
            permissions.set_mode(0o600);
            file.set_permissions(permissions)?;
            file.write_all(&seed)?;
            file.sync_all()?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_created_once_and_stable_across_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("node-signing.key");
        let a = load_or_create(&path).unwrap();
        let b = load_or_create(&path).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes(), "same key on reload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "sealed perms");
        }
    }

    #[test]
    fn corrupt_seed_is_refused_not_silently_regenerated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("node-signing.key");
        std::fs::write(&path, b"short").unwrap();
        assert!(
            load_or_create(&path).is_err(),
            "regenerating over a corrupt key would silently rotate the identity"
        );
    }

    #[test]
    fn oversized_seed_is_rejected_before_materialization() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("node-signing.key");
        std::fs::write(&path, [0_u8; NODE_SEED_BYTES + 1]).unwrap();
        assert_eq!(
            load_or_create(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[cfg(unix)]
    #[test]
    fn hostile_seed_leaves_are_not_followed_or_blocked() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside.key");
        std::fs::write(&outside, [7_u8; NODE_SEED_BYTES]).unwrap();

        let symlink_path = tmp.path().join("symlink.key");
        symlink(&outside, &symlink_path).unwrap();
        assert!(load_or_create(&symlink_path).is_err());

        let socket_path = tmp.path().join("socket.key");
        let _socket = UnixListener::bind(&socket_path).unwrap();
        assert!(load_or_create(&socket_path).is_err());
    }
}
