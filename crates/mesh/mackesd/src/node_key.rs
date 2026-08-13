//! SEC-6 — the persisted node signing key.
//!
//! A stable Ed25519 keypair per box (distinct from the per-enroll
//! `EnrolledIdentity`, which is minted fresh each enrollment, and
//! from the Nebula cert key, which stays single-purpose per §3).
//! Used to sign gossiped retract records so peers can attribute and
//! tamper-check revocations (Q28/29). Created on first use, sealed
//! at 0600.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Default on-disk location.
pub const DEFAULT_KEY_PATH: &str = "/var/lib/mackesd/node-signing.key";

const NODE_SEED_BYTES: usize = 32;

/// Installed, non-secret proof that the collaboration identity was admitted by
/// the governed release materializer. The detached release signature is
/// verified before this marker is emitted; runtime re-attests every field that
/// can change after materialization before granting publication authority.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollaborationIdentityAdmission {
    schema_version: u8,
    kind: String,
    public_key_hex: String,
    source_revision: String,
    target_node: String,
    target_user: String,
    seed_sha256: String,
}

/// Load the persisted signer only when its governed Collaboration admission
/// still matches this executable, node, user, and exact Ed25519 public key.
pub fn load_collaboration_admitted(
    key_path: &Path,
    admission_path: &Path,
    expected_node: &str,
    expected_user: &str,
    expected_revision: &str,
) -> io::Result<SigningKey> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let metadata = std::fs::symlink_metadata(admission_path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || (!cfg!(test) && metadata.uid() != 0)
        || metadata.permissions().mode() & 0o377 != 0
        || metadata.len() > 4096
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity admission is not a root-owned 0400 regular file",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(0o400_000 | 0o4_000 | 0o2_000_000); // O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
    let mut file = options.open(admission_path)?;
    let opened = file.metadata()?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity admission was replaced while being opened",
        ));
    }
    let mut body = Vec::with_capacity(4097);
    (&mut file).take(4097).read_to_end(&mut body)?;
    let after = file.metadata()?;
    let current = std::fs::symlink_metadata(admission_path)?;
    if after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || after.len() != opened.len()
        || after.mtime() != opened.mtime()
        || after.mtime_nsec() != opened.mtime_nsec()
        || current.dev() != metadata.dev()
        || current.ino() != metadata.ino()
        || current.len() != metadata.len()
        || current.mtime() != metadata.mtime()
        || current.mtime_nsec() != metadata.mtime_nsec()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity admission changed while being read",
        ));
    }
    let admission: CollaborationIdentityAdmission = serde_json::from_slice(&body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let exact_hex = |value: &str, bytes: usize| {
        value.len() == bytes * 2
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if admission.schema_version != 1
        || admission.kind != "mcnf-collaboration-identity-admission"
        || admission.target_node != expected_node
        || admission.target_user != expected_user
        || admission.source_revision != expected_revision
        || !exact_hex(&admission.public_key_hex, 32)
        || !exact_hex(&admission.seed_sha256, 32)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity admission is stale, malformed, or out of scope",
        ));
    }
    let seed = read_existing_seed(key_path)?;
    let actual_seed_sha256 = Sha256::digest(seed)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_seed_sha256 != admission.seed_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity seed does not match the admitted SecretStore material",
        ));
    }
    let key = SigningKey::from_bytes(&seed);
    let actual_public = key
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_public != admission.public_key_hex {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "collaboration identity does not match the admitted public key",
        ));
    }
    Ok(key)
}

/// Validate every existing parent component before opening or creating the
/// identity leaf. `O_NOFOLLOW` protects only the final leaf; a symlinked
/// `var/lib/mackesd` component could otherwise redirect the node key into an
/// operator-controlled or replicated tree.
fn validate_parent_chain(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "node signing key parent is a symlink: {}",
                        current.display()
                    ),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "node signing key parent is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Read an existing signing seed through a bounded descriptor boundary.
///
/// The seed is a security-sensitive replicated/runtime identity. Do not follow
/// a final symlink or open a special file that could block the daemon, and do
/// not let a growing or oversized leaf reach the key parser.
fn read_existing_seed(path: &Path) -> io::Result<[u8; NODE_SEED_BYTES]> {
    validate_parent_chain(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000 | 0o4_000 | 0o2_000_000); // O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
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
                validate_parent_chain(path)?;
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
    use std::os::unix::fs::PermissionsExt;

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
    fn collaboration_signer_requires_exact_release_admission() {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("node-signing.key");
        let admission_path = tmp.path().join("collaboration-admission.json");
        let key = load_or_create(&key_path).unwrap();
        let public = key
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let revision = "a".repeat(40);
        let body = serde_json::json!({
            "schema_version": 1,
            "kind": "mcnf-collaboration-identity-admission",
            "public_key_hex": public,
            "seed_sha256": Sha256::digest(key.to_bytes()).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "source_revision": revision,
            "target_node": "peer:seat-a",
            "target_user": "system:mackesd"
        });
        std::fs::write(&admission_path, serde_json::to_vec(&body).unwrap()).unwrap();
        std::fs::set_permissions(&admission_path, std::fs::Permissions::from_mode(0o400)).unwrap();
        load_collaboration_admitted(
            &key_path,
            &admission_path,
            "peer:seat-a",
            "system:mackesd",
            &revision,
        )
        .expect("exact release admission grants signer authority");

        for (node, user, source) in [
            ("peer:seat-b", "system:mackesd", revision.as_str()),
            ("peer:seat-a", "user:mm", revision.as_str()),
            (
                "peer:seat-a",
                "system:mackesd",
                "cccccccccccccccccccccccccccccccccccccccc",
            ),
        ] {
            assert!(
                load_collaboration_admitted(&key_path, &admission_path, node, user, source,)
                    .is_err()
            );
        }
        std::fs::write(&key_path, [9_u8; NODE_SEED_BYTES]).unwrap();
        assert!(load_collaboration_admitted(
            &key_path,
            &admission_path,
            "peer:seat-a",
            "system:mackesd",
            &revision,
        )
        .is_err());
        std::fs::write(&key_path, key.to_bytes()).unwrap();
        std::fs::set_permissions(&admission_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_collaboration_admitted(
            &key_path,
            &admission_path,
            "peer:seat-a",
            "system:mackesd",
            &revision,
        )
        .is_err());
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

    #[cfg(unix)]
    #[test]
    fn hostile_parent_directory_is_not_followed() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let redirected = tmp.path().join("identity");
        symlink(outside.path(), &redirected).unwrap();
        let path = redirected.join("node-signing.key");

        assert!(
            load_or_create(&path).is_err(),
            "a symlinked parent must not redirect node-key creation"
        );
        assert!(!outside.path().join("node-signing.key").exists());
    }
}
