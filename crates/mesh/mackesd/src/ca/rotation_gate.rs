//! SEC-2 (Q20) — the CA-rotation passphrase gate.
//!
//! Rotating the CA re-keys the whole mesh — the most consequential
//! admin action the platform has. Both rotation doors (`mackesd ca
//! rotate` and the `action/nebula/regen-certs` Bus verb) now require
//! the operator passphrase; promotion never rotates (it only mints
//! when no CA exists at all). The gate stores a SHA-256 hash on the
//! replicated root, set once via `mackesd ca set-passphrase`; an
//! unset gate refuses rotation with the set-it-first instruction —
//! fail closed, like ENT-2.

use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Verification outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCheck {
    /// Passphrase matches — rotation may proceed.
    Ok,
    /// Wrong passphrase.
    Wrong,
    /// No passphrase has been set — rotation refuses until one is.
    NotSet,
}

fn gate_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("ca").join("rotation-passphrase.hash")
}

fn hash(phrase: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"mde-ca-rotation-v1:");
    h.update(phrase.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Set (or change) the rotation passphrase. Changing requires the
/// current phrase via [`verify`] at the CLI layer.
///
/// # Errors
/// IO failures.
pub fn set_passphrase(workgroup_root: &Path, phrase: &str) -> io::Result<()> {
    let path = gate_path(workgroup_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, hash(phrase))
}

/// Check `phrase` against the stored gate.
#[must_use]
pub fn verify(workgroup_root: &Path, phrase: &str) -> GateCheck {
    match std::fs::read_to_string(gate_path(workgroup_root)) {
        Err(_) => GateCheck::NotSet,
        Ok(stored) => {
            // Constant-time-ish compare over fixed-length hex hashes.
            let candidate = hash(phrase);
            let stored = stored.trim();
            if stored.len() == candidate.len()
                && stored
                    .bytes()
                    .zip(candidate.bytes())
                    .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
            {
                GateCheck::Ok
            } else {
                GateCheck::Wrong
            }
        }
    }
}

/// The refusal copy both doors print — one message, one fix.
#[must_use]
pub fn refusal_message(check: GateCheck) -> Option<&'static str> {
    match check {
        GateCheck::Ok => None,
        GateCheck::Wrong => Some(
            "CA rotation refused: wrong passphrase (SEC-2). Rotation re-keys the whole mesh — \
             retry with the operator passphrase ($MDE_CA_PASSPHRASE or --passphrase-stdin).",
        ),
        GateCheck::NotSet => Some(
            "CA rotation refused: no rotation passphrase is set (SEC-2 fail-closed). Set one \
             first: MDE_CA_PASSPHRASE=… mackesd ca set-passphrase",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_fails_closed_then_verifies_then_refuses_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(verify(tmp.path(), "anything"), GateCheck::NotSet);
        set_passphrase(tmp.path(), "correct horse").unwrap();
        assert_eq!(verify(tmp.path(), "correct horse"), GateCheck::Ok);
        assert_eq!(verify(tmp.path(), "wrong"), GateCheck::Wrong);
        assert_eq!(verify(tmp.path(), ""), GateCheck::Wrong);
    }

    #[test]
    fn the_stored_gate_is_a_hash_not_the_phrase() {
        let tmp = tempfile::tempdir().unwrap();
        set_passphrase(tmp.path(), "hunter2").unwrap();
        let stored = std::fs::read_to_string(gate_path(tmp.path())).unwrap();
        assert!(!stored.contains("hunter2"));
        assert_eq!(stored.len(), 64);
    }

    #[test]
    fn refusal_copy_names_the_fix() {
        assert!(refusal_message(GateCheck::Ok).is_none());
        assert!(refusal_message(GateCheck::Wrong)
            .unwrap()
            .contains("wrong passphrase"));
        assert!(refusal_message(GateCheck::NotSet)
            .unwrap()
            .contains("set-passphrase"));
    }
}
