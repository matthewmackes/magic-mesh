//! SEC-8 (Q33/Q34) — KDC session keys, encrypted at rest.
//!
//! Session keys lived only in the in-memory `RingKeyStore`, so every
//! daemon restart killed the links. This module persists the
//! device→session-key map **sealed with AES-256-GCM** (Q33) under a
//! per-host master key (0600, created on first use): a restart
//! restores the sessions via [`crate::pairing::PairingStore`]'s open
//! path instead of forcing a re-pair. The plaintext never touches
//! disk; tampering or the wrong master key fails closed to an empty
//! map (a re-pair beats decrypting garbage).
//!
//! The live LAN/TLS handshake that *installs* fresh session keys is
//! SEC-4's scope — this layer is its persistence substrate.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::Path;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};

const MASTER_KEY_LEN: usize = 32;
// A session map is small in normal operation (one 32-byte key per device), but
// keep the encrypted envelope bounded before either AES or serde sees it.
const MAX_SESSION_FILE_BYTES: usize = 1024 * 1024;

fn invalid_file(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {reason}", path.display()),
    )
}

/// Read a stable, regular file through its opened descriptor.
///
/// The path check rejects an existing final symlink, while `O_NOFOLLOW` closes
/// the replacement race on Unix. The descriptor metadata check rejects a
/// special file or a path that changed between the path and descriptor
/// lookups. Reading one byte over the cap, then checking the descriptor size
/// again, rejects files that are oversized or grow while being read before
/// their contents reach decryption or deserialization.
fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let max_bytes_u64 = u64::try_from(max_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file size cap overflows u64"))?;
    let entry = std::fs::symlink_metadata(path)?;
    if !entry.file_type().is_file() {
        return Err(invalid_file(path, "not a regular file"));
    }
    if entry.len() > max_bytes_u64 {
        return Err(invalid_file(path, "file exceeds bounded read limit"));
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000); // O_NOFOLLOW
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100); // O_NOFOLLOW

        // Keep unsupported Unix targets fail-closed for symlink leaves even
        // when their standard library does not expose an O_NOFOLLOW value in
        // this crate's dependency surface.
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_file())
        {
            return Err(invalid_file(path, "not a regular file"));
        }
    }

    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file())
    {
        return Err(invalid_file(path, "not a regular file"));
    }

    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.file_type().is_file() {
        return Err(invalid_file(
            path,
            "opened descriptor is not a regular file",
        ));
    }
    if opened.len() > max_bytes_u64 {
        return Err(invalid_file(path, "file exceeds bounded read limit"));
    }
    let opened_len = opened.len();
    let capacity = usize::try_from(opened_len).unwrap_or(max_bytes);
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(max_bytes_u64.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(invalid_file(path, "file grew beyond bounded read limit"));
    }

    let after = file.metadata()?;
    if !after.file_type().is_file()
        || after.len() != opened_len
        || after.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    {
        return Err(invalid_file(path, "file changed while being read"));
    }
    Ok(bytes)
}

fn parse_master_key(path: &Path, bytes: &[u8]) -> io::Result<[u8; MASTER_KEY_LEN]> {
    if bytes.len() != MASTER_KEY_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a 32-byte master key", path.display()),
        ));
    }
    let mut key = [0_u8; MASTER_KEY_LEN];
    key.copy_from_slice(bytes);
    Ok(key)
}

fn create_master_key(path: &Path, key: &[u8; MASTER_KEY_LEN]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;
    }
    Ok(())
}

/// Load (or mint) the 32-byte master key, 0600.
///
/// # Errors
/// IO failures; a corrupt (wrong-length) master refuses rather than
/// silently rotating (persisted sessions would all be lost quietly).
pub fn load_or_create_master(path: &Path) -> io::Result<[u8; 32]> {
    match read_bounded_regular_file(path, MASTER_KEY_LEN) {
        Ok(bytes) => parse_master_key(path, &bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut key = [0_u8; MASTER_KEY_LEN];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| io::Error::other("CSPRNG failure"))?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            match create_master_key(path, &key) {
                Ok(()) => Ok(key),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let existing = read_bounded_regular_file(path, MASTER_KEY_LEN)?;
                    parse_master_key(path, &existing)
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// Seal + write the device→session-key map.
///
/// # Errors
/// IO / seal failures.
pub fn save_sessions(
    path: &Path,
    master: &[u8; 32],
    sessions: &BTreeMap<String, Vec<u8>>,
) -> io::Result<()> {
    let plain = serde_json::to_vec(sessions)?;
    let unbound =
        UnboundKey::new(&AES_256_GCM, master).map_err(|_| io::Error::other("bad master key"))?;
    let key = LessSafeKey::new(unbound);
    let mut nonce_bytes = [0_u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| io::Error::other("CSPRNG failure"))?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut buf = plain;
    key.seal_in_place_append_tag(nonce, Aad::from(b"mde-kdc-sessions-v1"), &mut buf)
        .map_err(|_| io::Error::other("seal failure"))?;
    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&buf);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("enc.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Load + unseal the map. Fails closed to empty on a missing file,
/// the wrong master, or any tampering — re-pairing beats garbage.
#[must_use]
pub fn load_sessions(path: &Path, master: &[u8; 32]) -> BTreeMap<String, Vec<u8>> {
    let Ok(raw) = read_bounded_regular_file(path, MAX_SESSION_FILE_BYTES) else {
        return BTreeMap::new();
    };
    if raw.len() < NONCE_LEN + 16 {
        return BTreeMap::new();
    }
    let (nonce_bytes, sealed) = raw.split_at(NONCE_LEN);
    let Ok(unbound) = UnboundKey::new(&AES_256_GCM, master) else {
        return BTreeMap::new();
    };
    let key = LessSafeKey::new(unbound);
    let mut nb = [0_u8; NONCE_LEN];
    nb.copy_from_slice(nonce_bytes);
    let nonce = Nonce::assume_unique_for_key(nb);
    let mut buf = sealed.to_vec();
    let Ok(plain) = key.open_in_place(nonce, Aad::from(b"mde-kdc-sessions-v1"), &mut buf) else {
        tracing::warn!(
            path = %path.display(),
            "SEC-8: sealed session store failed to open (wrong master / tampered) — \
             links will re-pair"
        );
        return BTreeMap::new();
    };
    serde_json::from_slice(plain).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BTreeMap<String, Vec<u8>> {
        let mut m = BTreeMap::new();
        m.insert("phone-a".to_string(), vec![7_u8; 32]);
        m.insert("tablet-b".to_string(), vec![9_u8; 32]);
        m
    }

    #[test]
    fn sessions_round_trip_sealed_and_survive_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let master = load_or_create_master(&tmp.path().join("master.key")).unwrap();
        let path = tmp.path().join("sessions.enc");
        save_sessions(&path, &master, &sample()).unwrap();
        // The Q34 acceptance: a "restart" (fresh load) restores them.
        assert_eq!(load_sessions(&path, &master), sample());
        // Plaintext keys never touch disk.
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !raw.windows(32).any(|w| w == [7_u8; 32]),
            "session key bytes must not appear in the sealed file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(tmp.path().join("master.key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn wrong_master_and_tampering_fail_closed_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let master = load_or_create_master(&tmp.path().join("m1.key")).unwrap();
        let path = tmp.path().join("sessions.enc");
        save_sessions(&path, &master, &sample()).unwrap();
        let other = load_or_create_master(&tmp.path().join("m2.key")).unwrap();
        assert!(load_sessions(&path, &other).is_empty(), "wrong master");
        let mut raw = std::fs::read(&path).unwrap();
        let len = raw.len();
        raw[len - 1] ^= 0xff;
        std::fs::write(&path, &raw).unwrap();
        assert!(load_sessions(&path, &master).is_empty(), "tampered");
    }

    #[test]
    fn corrupt_master_refuses_rather_than_rotating() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("master.key");
        std::fs::write(&path, b"short").unwrap();
        assert!(load_or_create_master(&path).is_err());
    }

    #[test]
    fn oversized_session_file_fails_closed_before_decrypting() {
        let tmp = tempfile::tempdir().unwrap();
        let master = [3_u8; MASTER_KEY_LEN];
        let path = tmp.path().join("sessions.enc");
        std::fs::write(&path, vec![0_u8; MAX_SESSION_FILE_BYTES + 1]).unwrap();
        assert!(load_sessions(&path, &master).is_empty());
    }

    #[test]
    fn malformed_session_input_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let master = [4_u8; MASTER_KEY_LEN];
        let path = tmp.path().join("sessions.enc");
        for invalid in [
            b"not a sealed session store".as_slice(),
            &[0_u8; NONCE_LEN + 16],
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(load_sessions(&path, &master).is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_is_rejected_for_session_and_master_reads() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let master_path = tmp.path().join("master.key");
        let master = load_or_create_master(&master_path).unwrap();
        let session_path = tmp.path().join("sessions.enc");
        save_sessions(&session_path, &master, &sample()).unwrap();

        let session_link = tmp.path().join("sessions-link.enc");
        symlink(&session_path, &session_link).unwrap();
        assert!(load_sessions(&session_link, &master).is_empty());

        let master_link = tmp.path().join("master-link.key");
        symlink(&master_path, &master_link).unwrap();
        assert!(load_or_create_master(&master_link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn special_file_is_rejected_for_session_and_master_reads() {
        let master = [5_u8; MASTER_KEY_LEN];
        assert!(load_sessions(Path::new("/dev/null"), &master).is_empty());
        assert!(load_or_create_master(Path::new("/dev/null")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn oversized_master_is_rejected_without_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("master.key");
        std::fs::write(&path, vec![0_u8; MASTER_KEY_LEN + 1]).unwrap();
        assert!(load_or_create_master(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap().len(), MASTER_KEY_LEN + 1);
    }
}
