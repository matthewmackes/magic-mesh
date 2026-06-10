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
use std::io;
use std::path::Path;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};

/// Load (or mint) the 32-byte master key, 0600.
///
/// # Errors
/// IO failures; a corrupt (wrong-length) master refuses rather than
/// silently rotating (persisted sessions would all be lost quietly).
pub fn load_or_create_master(path: &Path) -> io::Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0_u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(arr)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a 32-byte master key", path.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut key = [0_u8; 32];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| io::Error::other("CSPRNG failure"))?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, key)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(path)?.permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(path, perms)?;
            }
            Ok(key)
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
    let Ok(raw) = std::fs::read(path) else {
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
}
