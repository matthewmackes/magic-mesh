//! SEC-6 — the persisted node signing key.
//!
//! A stable Ed25519 keypair per box (distinct from the per-enroll
//! `EnrolledIdentity`, which is minted fresh each enrollment, and
//! from the Nebula cert key, which stays single-purpose per §3).
//! Used to sign gossiped retract records so peers can attribute and
//! tamper-check revocations (Q28/29). Created on first use, sealed
//! at 0600.

use std::io;
use std::path::Path;

use ed25519_dalek::SigningKey;

/// Default on-disk location.
pub const DEFAULT_KEY_PATH: &str = "/var/lib/mackesd/node-signing.key";

/// Load the node signing key, creating it on first use.
///
/// # Errors
/// IO failures (unreadable dir, bad permissions).
pub fn load_or_create(path: &Path) -> io::Result<SigningKey> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0_u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(SigningKey::from_bytes(&arr))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a 32-byte ed25519 seed", path.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            use rand::RngCore;
            let mut seed = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, seed)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(path)?.permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(path, perms)?;
            }
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
}
