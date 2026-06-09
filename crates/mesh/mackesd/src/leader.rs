//! Leader election via `~/QNM-Shared/.mackesd-leader.lock`
//! (Phase 12.1.1b).
//!
//! Every peer runs `mackesd`. The leader is the only writer to the
//! shared `desired_config` mirror. Election is filesystem-based via
//! an `fs2`-backed advisory lock with a 60-second lease. The
//! winning peer renews the lease every 20 seconds; on miss, the
//! next peer in lexicographic node-id order takes over.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lease duration. Per 12.A.5 lock: "60 s lease."
pub const LEASE_DURATION: Duration = Duration::from_secs(60);

/// Renewal cadence. Per 12.A.5 lock: "Lease renewal every 20 s."
pub const RENEW_INTERVAL: Duration = Duration::from_secs(20);

/// Outcome of a single `try_acquire` attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireResult {
    /// This peer acquired the lock and is now leader.
    Acquired,
    /// Another peer holds a fresh lease — we're a follower.
    HeldBy {
        /// Node id of the current leader.
        leader_id: String,
        /// Seconds remaining on their lease.
        lease_remaining_s: u64,
    },
    /// Lock file exists but the lease has expired — caller can
    /// retry immediately.
    ExpiredLease,
}

/// One leader-lease record persisted in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// Node id that owns this lease.
    pub node_id: String,
    /// Unix epoch seconds when the lease was renewed.
    pub renewed_at_s: u64,
    /// Epoch counter (bumps on every `take-leadership --force`).
    pub epoch: u64,
}

impl Lease {
    fn encode(&self) -> String {
        format!("{}\t{}\t{}\n", self.node_id, self.renewed_at_s, self.epoch)
    }

    fn decode(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            node_id: parts[0].to_owned(),
            renewed_at_s: parts[1].parse().ok()?,
            epoch: parts[2].parse().ok()?,
        })
    }

    /// Has this lease aged past `LEASE_DURATION`?
    #[must_use]
    pub fn is_expired(&self, now_s: u64) -> bool {
        now_s.saturating_sub(self.renewed_at_s) >= LEASE_DURATION.as_secs()
    }

    /// Remaining seconds on the lease (0 if expired).
    #[must_use]
    pub fn remaining_s(&self, now_s: u64) -> u64 {
        let age = now_s.saturating_sub(self.renewed_at_s);
        LEASE_DURATION.as_secs().saturating_sub(age)
    }
}

/// Try to acquire (or renew) the leader lease at `lock_path`.
/// The flow:
///
/// 1. `OpenOptions::create(true).write(true).read(true)` to get a
///    handle. The file is created if missing — every peer can open
///    it; the lock is what serializes writes.
/// 2. `fs2::try_lock_exclusive()` — non-blocking advisory lock. If
///    another peer holds it, we read the current lease + report
///    `HeldBy`.
/// 3. Read the existing lease. If it's `node_id`'s and not expired,
///    we already own it: bump `renewed_at` and write back.
/// 4. If expired, claim the lease (bump epoch + write `node_id` +
///    fresh `renewed_at`).
/// 5. Release the OS-level advisory lock once the lease is
///    persisted; the lease is what makes a peer "leader", not the
///    advisory lock.
///
/// # Errors
/// Returns `std::io::Error` when the lock file can't be opened or
/// rewritten.
pub fn try_acquire(lock_path: &Path, node_id: &str) -> std::io::Result<AcquireResult> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;

    // Advisory lock — non-blocking. If another peer is mid-write,
    // we report `HeldBy` based on the on-disk lease (which may be
    // a moment stale, but the follower retry loop handles that).
    if file.try_lock_exclusive().is_err() {
        let lease = read_lease(&mut file)?;
        return Ok(match lease {
            Some(l) => {
                let now = now_s();
                let remaining = l.remaining_s(now);
                AcquireResult::HeldBy {
                    leader_id: l.node_id,
                    lease_remaining_s: remaining,
                }
            }
            None => AcquireResult::ExpiredLease,
        });
    }

    let now = now_s();
    let existing = read_lease(&mut file)?;
    let next = match existing {
        None => Lease {
            node_id: node_id.to_owned(),
            renewed_at_s: now,
            epoch: 1,
        },
        Some(l) if l.node_id == node_id && !l.is_expired(now) => Lease {
            renewed_at_s: now,
            ..l
        },
        Some(l) if l.is_expired(now) => Lease {
            node_id: node_id.to_owned(),
            renewed_at_s: now,
            epoch: l.epoch + 1,
        },
        Some(l) => {
            // Another peer holds a fresh lease. Release our
            // advisory lock and surface them.
            let remaining = l.remaining_s(now);
            let _ = file.unlock();
            return Ok(AcquireResult::HeldBy {
                leader_id: l.node_id,
                lease_remaining_s: remaining,
            });
        }
    };

    write_lease(&mut file, &next)?;
    let _ = file.unlock();
    Ok(AcquireResult::Acquired)
}

/// Force this peer into leadership by writing a fresh lease with a
/// bumped epoch. Used by `mackesd take-leadership --force` (the
/// operator's last resort when automatic resolution wedges).
///
/// # Errors
/// Returns `std::io::Error` when the lock file can't be rewritten.
pub fn force_take(lock_path: &Path, node_id: &str) -> std::io::Result<Lease> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    let _ = file.lock_exclusive();
    let prior_epoch = read_lease(&mut file)?.map_or(0, |l| l.epoch);
    let next = Lease {
        node_id: node_id.to_owned(),
        renewed_at_s: now_s(),
        epoch: prior_epoch + 1,
    };
    write_lease(&mut file, &next)?;
    let _ = file.unlock();
    Ok(next)
}

fn read_lease(file: &mut File) -> std::io::Result<Option<Lease>> {
    file.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(Lease::decode(&text))
}

fn write_lease(file: &mut File, lease: &Lease) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    let body = lease.encode();
    file.write_all(body.as_bytes())?;
    file.sync_data()?;
    Ok(())
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let l = Lease {
            node_id: "peer:anvil".into(),
            renewed_at_s: 1_700_000_000,
            epoch: 7,
        };
        let text = l.encode();
        let back = Lease::decode(&text).expect("decode");
        assert_eq!(back, l);
    }

    #[test]
    fn decode_rejects_malformed_input() {
        assert!(Lease::decode("").is_none());
        assert!(Lease::decode("only-one-field").is_none());
        assert!(Lease::decode("a\tnot-a-number\t1").is_none());
    }

    #[test]
    fn expired_detection_uses_60s_threshold() {
        let l = Lease {
            node_id: "x".into(),
            renewed_at_s: 1_000,
            epoch: 1,
        };
        assert!(!l.is_expired(1_059));
        assert!(l.is_expired(1_060));
        assert!(l.is_expired(1_500));
    }

    #[test]
    fn remaining_zero_when_expired() {
        let l = Lease {
            node_id: "x".into(),
            renewed_at_s: 0,
            epoch: 1,
        };
        assert_eq!(l.remaining_s(1_000_000), 0);
    }

    #[test]
    fn try_acquire_creates_lease_when_lockfile_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leader.lock");
        let out = try_acquire(&path, "peer:a").unwrap();
        assert_eq!(out, AcquireResult::Acquired);
        // File should exist.
        assert!(path.exists());
    }

    #[test]
    fn try_acquire_renews_own_lease() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leader.lock");
        let _ = try_acquire(&path, "peer:a").unwrap();
        // Second attempt as same peer renews.
        let out = try_acquire(&path, "peer:a").unwrap();
        assert_eq!(out, AcquireResult::Acquired);
    }

    #[test]
    fn force_take_bumps_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leader.lock");
        let l1 = force_take(&path, "peer:a").unwrap();
        let l2 = force_take(&path, "peer:b").unwrap();
        assert_eq!(l2.epoch, l1.epoch + 1);
        assert_eq!(l2.node_id, "peer:b");
    }

    #[test]
    fn remaining_when_lease_valid_is_under_lease_duration() {
        let l = Lease {
            node_id: "x".into(),
            renewed_at_s: 1_000,
            epoch: 1,
        };
        // 5 seconds in → 55 remaining.
        assert_eq!(l.remaining_s(1_005), 55);
        // At t=1000, full 60s remain.
        assert_eq!(l.remaining_s(1_000), 60);
        // Past expiry → 0.
        assert_eq!(l.remaining_s(1_060), 0);
    }

    #[test]
    fn decode_rejects_unparseable_epoch() {
        // Right number of fields, but the trailing epoch is non-numeric.
        assert!(Lease::decode("peer:a\t1000\tnot-a-number").is_none());
    }

    #[test]
    fn force_take_creates_lease_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leader.lock");
        let l = force_take(&path, "peer:fresh").unwrap();
        assert_eq!(l.node_id, "peer:fresh");
        // No prior lease → epoch starts at 1.
        assert_eq!(l.epoch, 1);
    }

    #[test]
    fn try_acquire_creates_parent_directory() {
        // Caller supplies a path under a non-existent dir — the
        // function must mkdir -p before opening.
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c/leader.lock");
        let out = try_acquire(&nested, "peer:p").unwrap();
        assert_eq!(out, AcquireResult::Acquired);
        assert!(nested.exists());
    }

    #[test]
    fn lease_encode_format_is_tab_delimited() {
        let l = Lease {
            node_id: "peer:n".into(),
            renewed_at_s: 42,
            epoch: 7,
        };
        let encoded = l.encode();
        assert_eq!(encoded, "peer:n\t42\t7\n");
    }

    #[test]
    fn expired_at_exact_threshold_is_expired() {
        let l = Lease {
            node_id: "x".into(),
            renewed_at_s: 0,
            epoch: 1,
        };
        // The boundary check uses `>= LEASE_DURATION.as_secs()`.
        assert!(l.is_expired(60));
        assert!(!l.is_expired(59));
    }

    #[test]
    fn acquire_result_variant_equality() {
        let a = AcquireResult::Acquired;
        let b = AcquireResult::Acquired;
        assert_eq!(a, b);

        let h1 = AcquireResult::HeldBy {
            leader_id: "x".into(),
            lease_remaining_s: 10,
        };
        let h2 = AcquireResult::HeldBy {
            leader_id: "x".into(),
            lease_remaining_s: 10,
        };
        assert_eq!(h1, h2);
        assert_ne!(a, h1);
        assert_eq!(AcquireResult::ExpiredLease, AcquireResult::ExpiredLease);
    }
}
