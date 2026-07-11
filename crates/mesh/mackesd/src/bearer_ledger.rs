//! ENT-1 (C1) — the issued-but-unredeemed bearer ledger.
//!
//! The enforcement the docs always claimed: an enrollment bearer is
//! only honored when this ledger holds it as *issued and not yet
//! redeemed*, and redemption is **single-use** — the sign that
//! consumes it deletes it. The ledger stores **SHA-256 hashes**, not
//! raw bearers, so the (Syncthing-replicated) directory never carries
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

/// The note marker that scopes a bearer to a **lighthouse** join (set by
/// `add-peer --role lighthouse`). The CA-key-delivery gate (#12) keys on this.
pub const LIGHTHOUSE_ROLE_NOTE: &str = "role:lighthouse";

/// HA / turn-key (#12) — was `bearer` issued with a **lighthouse-role** scope? The
/// signer keys the CA-key delivery + Host-cert decision on this (the bearer note =
/// operator intent via `add-peer --role lighthouse`), NOT a self-asserted CSR
/// field. Requires the entry to be present (issued + unredeemed) AND its note to
/// carry [`LIGHTHOUSE_ROLE_NOTE`] — so a leaked/ordinary peer bearer can never pull
/// the CA private key (ENT-12 containment).
#[must_use]
pub fn is_lighthouse_bearer(workgroup_root: &Path, bearer: &str) -> bool {
    let path = ledger_dir(workgroup_root).join(format!("{}.json", hash_hex(bearer)));
    let Ok(body) = std::fs::read_to_string(&path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("note")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|note| note.contains(LIGHTHOUSE_ROLE_NOTE))
}

/// Atomically consume `bearer` for its single use — the ledger's
/// test-and-consume primitive (security-5).
///
/// Returns `true` to **exactly one** caller and `false` to every
/// other (never-issued, already-consumed, or lost a concurrent
/// race). Atomicity comes straight from `unlink(2)`: of N threads or
/// processes racing to remove the same ledger entry, the kernel lets
/// exactly one `remove_file` return `Ok` and the rest fail with
/// `ENOENT`. There is no check-then-act window inside this call — the
/// remove *is* the decision.
///
/// This is the single point that decides the single-use winner, so
/// callers MUST **gate bundle delivery on a `true` return** (deliver
/// iff you won the consume). Doing so closes the ENT-1 check-then-act
/// TOCTOU: two requests presenting the same bearer can both pass an
/// [`is_pending`] pre-check, but only one wins `consume`, so only one
/// enrollment bundle is ever delivered. Consume BEFORE delivering, so
/// two racers never both write a peer's shared bundle path — only the
/// winner proceeds.
#[must_use]
pub fn consume(workgroup_root: &Path, bearer: &str) -> bool {
    std::fs::remove_file(ledger_dir(workgroup_root).join(format!("{}.json", hash_hex(bearer))))
        .is_ok()
}

/// Redeem `bearer` — single-use: returns `true` exactly once per
/// issued bearer (the entry is deleted). Historical name for, and
/// identical to, [`consume`] (the atomic test-and-consume).
#[must_use]
pub fn redeem(workgroup_root: &Path, bearer: &str) -> bool {
    consume(workgroup_root, bearer)
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
    fn consume_is_single_use_and_refuses_spent_or_unknown() {
        // security-5: the atomic consume — a normal single redeem
        // wins once; an already-consumed bearer and a never-issued
        // one are both refused.
        let tmp = tempfile::tempdir().unwrap();
        let bearer = issue(tmp.path(), "box").unwrap();
        assert!(
            consume(tmp.path(), &bearer),
            "first consume of an issued bearer wins"
        );
        assert!(
            !consume(tmp.path(), &bearer),
            "an already-consumed bearer is refused (single-use)"
        );
        assert!(
            !consume(tmp.path(), "never-issued"),
            "a never-issued bearer is refused"
        );
    }

    #[test]
    fn concurrent_consumers_of_one_bearer_have_exactly_one_winner() {
        // security-5 acceptance: hammer a single issued bearer with N
        // threads that all try to consume it at the same instant. The
        // unlink-based test-and-consume must let EXACTLY ONE win — the
        // TOCTOU the old split is_pending/redeem lost, where two racers
        // both passed the check and both got honored.
        use std::sync::{Arc, Barrier};

        let tmp = tempfile::tempdir().unwrap();
        let bearer = issue(tmp.path(), "race box").unwrap();

        const N: usize = 64;
        let barrier = Arc::new(Barrier::new(N));
        let root = Arc::new(tmp.path().to_path_buf());
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let barrier = Arc::clone(&barrier);
            let root = Arc::clone(&root);
            let bearer = bearer.clone();
            handles.push(std::thread::spawn(move || {
                // Release all threads together to maximize contention
                // on the one ledger entry.
                barrier.wait();
                consume(&root, &bearer)
            }));
        }
        let wins = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(
            wins, 1,
            "exactly one racing redeemer may win the single-use bearer"
        );
        // The bearer is now spent for good — no later consume can win.
        assert!(
            !consume(root.as_path(), &bearer),
            "a spent bearer is refused after the race"
        );
        assert!(
            !is_pending(root.as_path(), &bearer),
            "the spent entry is gone"
        );
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

    #[test]
    fn lighthouse_bearer_is_recognized_only_by_its_role_note() {
        let tmp = tempfile::tempdir().unwrap();
        let lh = issue(tmp.path(), &format!("{LIGHTHOUSE_ROLE_NOTE} sfo3 box")).unwrap();
        let peer = issue(tmp.path(), "plain workstation").unwrap();
        assert!(
            is_lighthouse_bearer(tmp.path(), &lh),
            "a role:lighthouse-noted bearer → true"
        );
        assert!(
            !is_lighthouse_bearer(tmp.path(), &peer),
            "an ordinary peer bearer → false (can't pull the CA key)"
        );
        assert!(
            !is_lighthouse_bearer(tmp.path(), "made-up"),
            "an absent bearer → false"
        );
        // A redeemed lighthouse bearer is no longer recognized (entry deleted).
        assert!(redeem(tmp.path(), &lh));
        assert!(
            !is_lighthouse_bearer(tmp.path(), &lh),
            "a spent bearer → false (single-use)"
        );
    }
}
