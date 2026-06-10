//! ENT-1 (C1) — the issued-but-unredeemed bearer ledger.
//!
//! The enforcement the docs always claimed: an enrollment bearer is
//! only honored when this ledger holds it as *issued and not yet
//! redeemed*, and redemption is **single-use** — the sign that
//! consumes it deletes it. The ledger stores **SHA-256 hashes**, not
//! raw bearers, so the (LizardFS-replicated) directory never carries
//! a usable token; possession of the raw bearer stays with whoever
//! the operator handed the join token to.
//!
//! Bearers minted here are 32 CSPRNG bytes, URL-safe base64 — the
//! SEC-3 256-bit strength, replacing the legacy 16-char passcode as
//! the thing a join token carries.

use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

/// The ledger directory under the CA state root.
#[must_use]
pub fn ledger_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("ca").join("issued-bearers")
}

fn hash_hex(bearer: &str) -> String {
    let mut h = Sha256::new();
    h.update(bearer.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Mint a fresh 256-bit bearer, record its hash as issued, and
/// return the raw bearer (shown once — it is never stored).
///
/// # Errors
/// IO failures writing the ledger entry.
pub fn issue(workgroup_root: &Path, note: &str) -> io::Result<String> {
    use rand::RngCore;
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let bearer = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let dir = ledger_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let entry = serde_json::json!({
        "issued_at_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
        "note": note,
    });
    std::fs::write(
        dir.join(format!("{}.json", hash_hex(&bearer))),
        entry.to_string(),
    )?;
    Ok(bearer)
}

/// Record an externally-supplied bearer as issued (the migration /
/// test seam — normal minting goes through [`issue`]).
///
/// # Errors
/// IO failures writing the ledger entry.
pub fn record_issued(workgroup_root: &Path, bearer: &str) -> io::Result<()> {
    let dir = ledger_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{}.json", hash_hex(bearer))),
        "{\"issued_at_ms\":0,\"note\":\"recorded\"}",
    )
}

/// Is `bearer` issued and not yet redeemed?
#[must_use]
pub fn is_pending(workgroup_root: &Path, bearer: &str) -> bool {
    ledger_dir(workgroup_root)
        .join(format!("{}.json", hash_hex(bearer)))
        .exists()
}

/// Redeem `bearer` — single-use: returns `true` exactly once per
/// issued bearer (the entry is deleted).
#[must_use]
pub fn redeem(workgroup_root: &Path, bearer: &str) -> bool {
    std::fs::remove_file(ledger_dir(workgroup_root).join(format!("{}.json", hash_hex(bearer))))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_bearers_are_256_bit_pending_and_single_use() {
        let tmp = tempfile::tempdir().unwrap();
        let bearer = issue(tmp.path(), "test box").unwrap();
        // 32 bytes URL-safe-no-pad base64 = 43 chars.
        assert_eq!(bearer.len(), 43, "256-bit strength (SEC-3)");
        assert!(is_pending(tmp.path(), &bearer));
        assert!(redeem(tmp.path(), &bearer), "first redemption succeeds");
        assert!(!is_pending(tmp.path(), &bearer), "spent");
        assert!(!redeem(tmp.path(), &bearer), "replay refused (single-use)");
    }

    #[test]
    fn unknown_and_absent_bearers_are_never_pending() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_pending(tmp.path(), "made-up"));
        assert!(!is_pending(tmp.path(), ""));
        assert!(!redeem(tmp.path(), "made-up"));
    }

    #[test]
    fn ledger_stores_hashes_not_raw_bearers() {
        let tmp = tempfile::tempdir().unwrap();
        let bearer = issue(tmp.path(), "n").unwrap();
        let entries: Vec<String> = std::fs::read_dir(ledger_dir(tmp.path()))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(
            !entries[0].contains(&bearer),
            "the replicated ledger must never carry a usable token"
        );
    }
}
