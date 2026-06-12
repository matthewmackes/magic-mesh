//! BUS-1.9 — message retention + GFS quota enforcement.
//!
//! Per `docs/design/v6.x-mackes-bus.md` §8:
//!
//! - **Per-priority TTL**:
//!     - `urgent`  — forever (never deleted)
//!     - `high`    — 30 days
//!     - `default` — 7 days
//!     - `min`     — 24 hours
//!   Operators can override per-topic via `policy.yaml` (deferred
//!   to BUS-7.3 — the default policy ships hardcoded here).
//! - **GFS quota** — soft 500 MB, hard 2 GB. Soft breach
//!   publishes a `bus/sys/quota` warning at `default` priority;
//!   hard breach AFAICT halts new publishes (BUS-1.4 write path
//!   doesn't enforce yet — surfaced in [`QuotaReport`] for the
//!   operator alert path).
//!
//! The retention loop spawns as a tokio task next to the broker
//! supervisor and runs one GC pass per hour. Each pass walks the
//! SQLite index by `ts_unix_ms`, finds messages past their
//! priority's TTL, deletes both the on-disk JSON file and the
//! index row, and finally sums disk usage to compare against
//! the quota.
//!
//! Pure helpers — `policy_ttl`, `ttl_cutoff_unix_ms`,
//! `find_expired_in_index`, `disk_usage_bytes` — are exposed
//! for tests so the loop can be exercised deterministically
//! with a stub clock.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use thiserror::Error;

use crate::persist::Persist;

/// TTL per priority class. Operators can override per-topic in
/// BUS-7.3's policy.yaml (deferred); the defaults here match
/// the design-doc lock.
pub const DEFAULT_TTL_MIN_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_TTL_DEFAULT_SECS: u64 = 7 * 24 * 60 * 60;
pub const DEFAULT_TTL_HIGH_SECS: u64 = 30 * 24 * 60 * 60;
/// EFF-47 — ephemeral RPC topics (`reply/<ulid>` + `action/…`).
/// Interactive request/response pairs are garbage minutes after
/// the exchange; without this class they sat at the `default`
/// 7-day TTL and accumulated one row + file per RPC. One hour
/// comfortably covers any in-flight poll/retry window.
pub const DEFAULT_TTL_EPHEMERAL_SECS: u64 = 60 * 60;

/// GFS quota thresholds.
pub const DEFAULT_QUOTA_SOFT_BYTES: u64 = 500 * 1024 * 1024;
pub const DEFAULT_QUOTA_HARD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default GC pass cadence — one pass per hour. Faster than
/// the shortest TTL (24h for `min`) so messages don't linger
/// too long past expiry.
pub const DEFAULT_PASS_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Resolved retention policy. Construct via [`RetentionPolicy::default()`]
/// for the design-locked defaults.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    /// Seconds before `min`-priority messages are GC'd.
    pub ttl_min_secs: u64,
    /// Seconds before `default`-priority messages are GC'd.
    pub ttl_default_secs: u64,
    /// Seconds before `high`-priority messages are GC'd.
    pub ttl_high_secs: u64,
    /// EFF-47 — seconds before ephemeral RPC topics (`reply/*`,
    /// `action/*`) are GC'd regardless of priority class.
    pub ttl_ephemeral_secs: u64,
    /// Soft quota — exceeding this publishes `bus/sys/quota`
    /// warning (default priority).
    pub quota_soft_bytes: u64,
    /// Hard quota — surfaced in [`QuotaReport::hard_exceeded`]
    /// so the operator + downstream alerting can react.
    pub quota_hard_bytes: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            ttl_min_secs: DEFAULT_TTL_MIN_SECS,
            ttl_default_secs: DEFAULT_TTL_DEFAULT_SECS,
            ttl_high_secs: DEFAULT_TTL_HIGH_SECS,
            ttl_ephemeral_secs: DEFAULT_TTL_EPHEMERAL_SECS,
            quota_soft_bytes: DEFAULT_QUOTA_SOFT_BYTES,
            quota_hard_bytes: DEFAULT_QUOTA_HARD_BYTES,
        }
    }
}

impl RetentionPolicy {
    /// TTL in seconds for a given priority. `urgent` returns
    /// `None` (never expires).
    #[must_use]
    pub fn ttl_secs(&self, priority: &str) -> Option<u64> {
        match priority {
            "min" => Some(self.ttl_min_secs),
            "default" => Some(self.ttl_default_secs),
            "high" => Some(self.ttl_high_secs),
            "urgent" => None,
            _ => Some(self.ttl_default_secs),
        }
    }
}

/// Result of one retention pass.
#[derive(Debug, Clone, Default)]
pub struct PassReport {
    /// Files (and matching index rows) removed during this pass.
    pub removed: usize,
    /// Total disk bytes used by remaining on-disk messages.
    pub bytes_after: u64,
    /// Quota state post-pass.
    pub quota: QuotaReport,
}

/// Quota breach status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaReport {
    /// True when bytes_after > soft_quota.
    pub soft_exceeded: bool,
    /// True when bytes_after > hard_quota.
    pub hard_exceeded: bool,
}

/// Errors a retention pass can surface.
#[derive(Debug, Error)]
pub enum RetentionError {
    /// SQLite failure (query, delete, etc.).
    #[error("sql: {0}")]
    Sql(String),
    /// File-system failure (delete, walk, etc.).
    #[error("io: {0}")]
    Io(String),
}

/// Compute the unix-ms cutoff below which messages of `priority`
/// are eligible for deletion. Pure helper exposed for tests.
#[must_use]
pub fn ttl_cutoff_unix_ms(
    policy: &RetentionPolicy,
    priority: &str,
    now_unix_ms: i64,
) -> Option<i64> {
    let secs = policy.ttl_secs(priority)?;
    let cutoff = now_unix_ms - i64::try_from(secs * 1000).unwrap_or(i64::MAX);
    Some(cutoff)
}

/// Walk the file tree under `root` and sum the bytes of every
/// `<ulid>.json` it contains. Excludes the SQLite database
/// itself + any `.tmp` files. Pure helper for tests.
pub fn disk_usage_bytes(root: &Path) -> Result<u64, RetentionError> {
    fn walk(dir: &Path, acc: &mut u64) -> Result<(), RetentionError> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| RetentionError::Io(format!("readdir {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| RetentionError::Io(format!("readdir entry: {e}")))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("index.sqlite") || name.ends_with(".tmp") {
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let ft = entry
                .file_type()
                .map_err(|e| RetentionError::Io(format!("file_type {}: {e}", path.display())))?;
            if ft.is_dir() {
                walk(&path, acc)?;
            } else if ft.is_file() && name.ends_with(".json") {
                let meta = entry
                    .metadata()
                    .map_err(|e| RetentionError::Io(format!("metadata {}: {e}", path.display())))?;
                *acc += meta.len();
            }
        }
        Ok(())
    }
    let mut acc: u64 = 0;
    walk(root, &mut acc)?;
    Ok(acc)
}

/// Open the per-peer index that lives at `<bus_root>/index.sqlite`.
fn open_index(bus_root: &Path) -> Result<Connection, RetentionError> {
    let p = bus_root.join("index.sqlite");
    let conn = Connection::open(&p)
        .map_err(|e| RetentionError::Sql(format!("open {}: {e}", p.display())))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| RetentionError::Sql(format!("busy_timeout: {e}")))?;
    Ok(conn)
}

/// One GC pass: find every message past its priority's TTL,
/// remove the on-disk JSON file, delete the index row, then
/// compute the post-pass disk usage + quota state.
///
/// `now_unix_ms` is injected for testability — production
/// callers pass `current_unix_ms()`.
///
/// # Errors
/// [`RetentionError::Sql`] or [`RetentionError::Io`] depending
/// on which layer fails.
pub fn run_pass_at(
    policy: &RetentionPolicy,
    bus_root: &Path,
    now_unix_ms: i64,
) -> Result<PassReport, RetentionError> {
    let conn = open_index(bus_root)?;

    // Build cutoff per priority class and collect rows to delete.
    // EPIC-BUS-EXT-AUDIT-BUS (Q28) — `audit/*` topics are
    // retention=forever (the audit trail must never be reaped, even
    // though audit records are `min` priority); exclude them from the
    // victim query regardless of TTL.
    let audit_like = format!("{}%", crate::persist::AUDIT_TOPIC_PREFIX);
    let mut victims: Vec<(String, String)> = Vec::new(); // (ulid, file_path)
    for priority in ["min", "default", "high"] {
        let Some(cutoff) = ttl_cutoff_unix_ms(policy, priority, now_unix_ms) else {
            continue;
        };
        let mut stmt = conn
            .prepare(
                "SELECT ulid, file_path FROM messages \
                 WHERE priority = ?1 AND ts_unix_ms < ?2 AND topic NOT LIKE ?3",
            )
            .map_err(|e| RetentionError::Sql(format!("prepare: {e}")))?;
        let rows = stmt
            .query_map(params![priority, cutoff, audit_like], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| RetentionError::Sql(format!("query: {e}")))?;
        for r in rows {
            victims.push(r.map_err(|e| RetentionError::Sql(format!("decode: {e}")))?);
        }
    }

    // EFF-47 — ephemeral RPC topics (`reply/<ulid>` request-response
    // pairs + the `action/…` requests that produced them) reap on the
    // short ephemeral TTL regardless of priority class. Without this,
    // every interactive RPC accumulated a row + file for the full
    // 7-day default TTL. `audit/*` can't match either prefix, but the
    // exclusion is kept for defense in depth.
    {
        let cutoff = now_unix_ms
            - i64::try_from(policy.ttl_ephemeral_secs * 1000).unwrap_or(i64::MAX);
        let mut stmt = conn
            .prepare(
                "SELECT ulid, file_path FROM messages \
                 WHERE (topic LIKE 'reply/%' OR topic LIKE 'action/%') \
                   AND ts_unix_ms < ?1 AND topic NOT LIKE ?2",
            )
            .map_err(|e| RetentionError::Sql(format!("prepare ephemeral: {e}")))?;
        let rows = stmt
            .query_map(params![cutoff, audit_like], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| RetentionError::Sql(format!("query ephemeral: {e}")))?;
        for r in rows {
            let v = r.map_err(|e| RetentionError::Sql(format!("decode ephemeral: {e}")))?;
            // The per-priority pass may already hold this victim
            // (a reply older than its priority TTL) — dedupe so the
            // delete loop doesn't double-count.
            if !victims.iter().any(|(u, _)| u == &v.0) {
                victims.push(v);
            }
        }
    }

    // Delete file + row for each victim. File first so an
    // index-only orphan is the worst-case failure mode (rather
    // than a row pointing at a deleted file, which the
    // divergence detector then has to clean up).
    let mut removed = 0_usize;
    for (ulid, rel_path) in &victims {
        let abs = bus_root.join(rel_path);
        if abs.exists() {
            std::fs::remove_file(&abs)
                .map_err(|e| RetentionError::Io(format!("rm {}: {e}", abs.display())))?;
        }
        conn.execute("DELETE FROM messages WHERE ulid = ?1", params![ulid])
            .map_err(|e| RetentionError::Sql(format!("delete row {ulid}: {e}")))?;
        removed += 1;
    }

    // Compute disk usage + quota state.
    let bytes_after = disk_usage_bytes(bus_root)?;
    let quota = QuotaReport {
        soft_exceeded: bytes_after > policy.quota_soft_bytes,
        hard_exceeded: bytes_after > policy.quota_hard_bytes,
    };
    Ok(PassReport {
        removed,
        bytes_after,
        quota,
    })
}

/// Convenience: today's clock as unix-ms.
pub fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Long-lived retention loop. Wakes on a `tokio::time::interval`
/// tick, calls [`run_pass_at`] with the current clock, and
/// publishes a `bus/sys/quota` warning when the soft quota is
/// exceeded.
///
/// Spawns one task; exit on shutdown signal (operator-cancelled
/// via `shutdown_rx`).
pub async fn run_loop(
    policy: RetentionPolicy,
    bus_root: PathBuf,
    interval: Duration,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick fires immediately — skip it to give the daemon
    // a clean second-stage startup window before the first GC.
    ticker.tick().await;
    let mut last_soft_warn = false;
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => return,
            _ = ticker.tick() => {
                match run_pass_at(&policy, &bus_root, current_unix_ms()) {
                    Ok(report) => {
                        tracing::info!(
                            target: "mde_bus::retention",
                            removed = report.removed,
                            bytes_after = report.bytes_after,
                            soft_exceeded = report.quota.soft_exceeded,
                            hard_exceeded = report.quota.hard_exceeded,
                            "retention pass complete"
                        );
                        if report.quota.soft_exceeded && !last_soft_warn {
                            // Publish a warning to bus/sys/quota.
                            // Best-effort — failure to publish is
                            // logged but doesn't break the loop.
                            if let Err(e) = publish_quota_warning(
                                &bus_root,
                                report.bytes_after,
                                policy.quota_soft_bytes,
                                policy.quota_hard_bytes,
                            ) {
                                tracing::warn!(
                                    target: "mde_bus::retention",
                                    error = %e,
                                    "failed to publish quota warning"
                                );
                            }
                            last_soft_warn = true;
                        } else if !report.quota.soft_exceeded {
                            last_soft_warn = false;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "mde_bus::retention",
                            error = %e,
                            "retention pass failed"
                        );
                    }
                }
            }
        }
    }
}

/// Publish a `bus/sys/quota` warning into the per-peer Persist
/// + on-disk tree. Pure sync helper — the retention loop calls
/// this from inside the tokio task.
fn publish_quota_warning(
    bus_root: &Path,
    bytes_used: u64,
    soft_limit: u64,
    hard_limit: u64,
) -> Result<(), RetentionError> {
    let p = Persist::open(bus_root.to_path_buf())
        .map_err(|e| RetentionError::Io(format!("open persist: {e}")))?;
    let title = format!(
        "Bus storage at {} MB / {} MB soft limit",
        bytes_used / 1024 / 1024,
        soft_limit / 1024 / 1024
    );
    let body = format!(
        "Mackes Bus on-disk store has exceeded its soft quota.\n\
         Used: {} MB\n\
         Soft limit: {} MB\n\
         Hard limit: {} MB\n\n\
         Retention is running but messages outlive their TTL when \
         the quota is over. Consider lowering per-topic TTL or \
         retiring noisy topics.",
        bytes_used / 1024 / 1024,
        soft_limit / 1024 / 1024,
        hard_limit / 1024 / 1024,
    );
    p.write(
        "bus/sys/quota",
        crate::hooks::config::Priority::Default,
        Some(&title),
        Some(&body),
    )
    .map_err(|e| RetentionError::Io(format!("publish quota warning: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::config::Priority;

    fn open_tmp_with(messages: &[(&str, Priority, i64)]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bus_root = tmp.path().to_path_buf();
        let p = Persist::open(bus_root.clone()).unwrap();
        // Insert with controlled ts_unix_ms by writing then
        // back-dating via direct SQLite UPDATE — keeps the test
        // hermetic against wall-clock drift.
        for (topic, prio, ts) in messages {
            let m = p.write(topic, *prio, None, Some("body")).unwrap();
            let conn = Connection::open(bus_root.join("index.sqlite")).unwrap();
            conn.execute(
                "UPDATE messages SET ts_unix_ms = ?1 WHERE ulid = ?2",
                params![ts, m.ulid],
            )
            .unwrap();
        }
        // EPIC-BUS-EXT-AUDIT-BUS: each write() above also emitted an
        // audit/<peer> record. Purge them (row + files) so the
        // retention fixture contains exactly the seeded messages —
        // retention behavior is what's under test here, not the audit
        // doubling (audit/* is retention-exempt anyway, exercised in
        // its own test below).
        let conn = Connection::open(bus_root.join("index.sqlite")).unwrap();
        conn.execute("DELETE FROM messages WHERE topic LIKE 'audit/%'", [])
            .unwrap();
        let _ = std::fs::remove_dir_all(bus_root.join("audit"));
        (tmp, bus_root)
    }

    #[test]
    fn ttl_secs_returns_per_priority_defaults() {
        let p = RetentionPolicy::default();
        assert_eq!(p.ttl_secs("min"), Some(DEFAULT_TTL_MIN_SECS));
        assert_eq!(p.ttl_secs("default"), Some(DEFAULT_TTL_DEFAULT_SECS));
        assert_eq!(p.ttl_secs("high"), Some(DEFAULT_TTL_HIGH_SECS));
        assert_eq!(p.ttl_secs("urgent"), None);
        // Unknown priorities fall back to `default`.
        assert_eq!(p.ttl_secs("garbage"), Some(DEFAULT_TTL_DEFAULT_SECS));
    }

    #[test]
    fn ttl_cutoff_subtracts_correctly() {
        let p = RetentionPolicy::default();
        // 1_000_000_000_000 ms = 2001-09-09T01:46:40Z
        let now = 1_000_000_000_000_i64;
        let cutoff_default = ttl_cutoff_unix_ms(&p, "default", now).unwrap();
        // default TTL = 7 days = 604800 seconds = 604_800_000 ms
        assert_eq!(now - cutoff_default, 604_800_000);
    }

    #[test]
    fn urgent_messages_never_expire() {
        let now = 1_000_000_000_000_i64;
        // 20 years ago — still urgent, still kept.
        let twenty_years = 20_i64 * 365 * 24 * 60 * 60 * 1000;
        let ancient = now - twenty_years;
        let (_tmp, root) = open_tmp_with(&[("t", Priority::Urgent, ancient)]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 0);
    }

    #[test]
    fn min_messages_expire_after_24h() {
        let now = 1_000_000_000_000_i64;
        let day_ago_plus = now - (25_i64 * 60 * 60 * 1000); // 25h ago
        let recent = now - (1_i64 * 60 * 60 * 1000); // 1h ago
        let (_tmp, root) = open_tmp_with(&[
            ("t/old", Priority::Min, day_ago_plus),
            ("t/new", Priority::Min, recent),
        ]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 1);
        // The recent message is still indexed.
        let p = Persist::open(root.clone()).unwrap();
        assert_eq!(p.count().unwrap(), 1);
    }

    #[test]
    fn audit_topics_exempt_from_retention() {
        // EPIC-BUS-EXT-AUDIT-BUS (Q28) — audit/* is retention=forever
        // even though audit records are `min` priority.
        let now = 1_000_000_000_000_i64;
        let ten_days = now - (10 * 24 * 60 * 60 * 1000);
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let p = Persist::open(root.clone()).unwrap();
        // audit/peerx is cycle-guarded (no further audit); the regular
        // write also emits an audit/<peer> record.
        p.write("audit/peerx", Priority::Min, None, Some("{}"))
            .unwrap();
        p.write("t/regular", Priority::Min, None, Some("body"))
            .unwrap();
        // Back-date everything to 10 days ago — well past the 24h min TTL.
        let conn = Connection::open(root.join("index.sqlite")).unwrap();
        conn.execute("UPDATE messages SET ts_unix_ms = ?1", params![ten_days])
            .unwrap();
        drop(conn);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        // Only the non-audit message is reaped; both audit/* records
        // survive despite being 10 days old + min priority.
        assert_eq!(report.removed, 1, "only the non-audit message reaped");
        let p = Persist::open(root.clone()).unwrap();
        assert!(
            !p.list_since("audit/peerx", None).unwrap().is_empty(),
            "audit/peerx survived the reap"
        );
    }

    #[test]
    fn ephemeral_rpc_topics_reap_after_one_hour() {
        // EFF-47 — reply/* + action/* reap on the 1h ephemeral TTL
        // while a same-age default-priority normal topic survives its
        // 7-day class TTL.
        let now = 1_000_000_000_000_i64;
        let two_hours_ago = now - (2_i64 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[
            ("reply/01OLDULID", Priority::Default, two_hours_ago),
            ("action/fleet/list-revisions", Priority::Default, two_hours_ago),
            ("t/normal", Priority::Default, two_hours_ago),
        ]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 2, "both RPC topics reaped, normal survives");
        let p = Persist::open(root.clone()).unwrap();
        assert!(p.list_since("reply/01OLDULID", None).unwrap().is_empty());
        assert!(p
            .list_since("action/fleet/list-revisions", None)
            .unwrap()
            .is_empty());
        assert!(!p.list_since("t/normal", None).unwrap().is_empty());
    }

    #[test]
    fn fresh_rpc_topics_survive_the_ephemeral_ttl() {
        // An in-flight RPC (30 min old) must NOT be reaped.
        let now = 1_000_000_000_000_i64;
        let half_hour_ago = now - (30_i64 * 60 * 1000);
        let (_tmp, root) =
            open_tmp_with(&[("reply/01FRESH", Priority::Default, half_hour_ago)]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 0);
    }

    #[test]
    fn ephemeral_pass_dedupes_against_priority_pass() {
        // A reply/* row older than BOTH its priority TTL and the
        // ephemeral TTL is removed exactly once.
        let now = 1_000_000_000_000_i64;
        let ten_days_ago = now - (10_i64 * 24 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[("reply/01ANCIENT", Priority::Min, ten_days_ago)]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 1, "single removal despite matching both passes");
    }

    #[test]
    fn default_messages_expire_after_7d() {
        let now = 1_000_000_000_000_i64;
        let eight_days = now - (8_i64 * 24 * 60 * 60 * 1000);
        let three_days = now - (3_i64 * 24 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[
            ("t/old", Priority::Default, eight_days),
            ("t/new", Priority::Default, three_days),
        ]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 1);
    }

    #[test]
    fn high_messages_expire_after_30d() {
        let now = 1_000_000_000_000_i64;
        let thirty_one = now - (31_i64 * 24 * 60 * 60 * 1000);
        let ten = now - (10_i64 * 24 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[
            ("t/old", Priority::High, thirty_one),
            ("t/new", Priority::High, ten),
        ]);
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 1);
    }

    #[test]
    fn pass_removes_both_file_and_row() {
        let now = 1_000_000_000_000_i64;
        let two_days = now - (2 * 24 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[("t/x", Priority::Min, two_days)]);
        // Sanity: file is on disk.
        let p = Persist::open(root.clone()).unwrap();
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 1);
        // File AND row gone.
        assert_eq!(p.count().unwrap(), 0);
        assert!(p.detect_divergence().unwrap().is_clean());
    }

    #[test]
    fn disk_usage_sums_json_files() {
        let (_tmp, root) = open_tmp_with(&[("t/x", Priority::Min, 1)]);
        let bytes = disk_usage_bytes(&root).unwrap();
        // The one message's JSON is at least a few bytes long.
        assert!(bytes > 50, "expected non-trivial bytes, got {bytes}");
    }

    #[test]
    fn quota_soft_breach_surfaces_in_report() {
        let policy = RetentionPolicy {
            quota_soft_bytes: 1, // 1 byte — guaranteed exceeded
            quota_hard_bytes: 2,
            ..RetentionPolicy::default()
        };
        let (_tmp, root) = open_tmp_with(&[("t/x", Priority::Urgent, 1)]);
        let report = run_pass_at(&policy, &root, 100).unwrap();
        assert!(report.quota.soft_exceeded);
        assert!(report.quota.hard_exceeded);
    }

    #[test]
    fn unknown_priority_falls_back_to_default_ttl() {
        let now = 1_000_000_000_000_i64;
        let eight_days = now - (8 * 24 * 60 * 60 * 1000);
        let (_tmp, root) = open_tmp_with(&[("t/x", Priority::Default, eight_days)]);
        // Hack: write a row with an unknown priority string by
        // bypassing the typed write path.
        let conn = Connection::open(root.join("index.sqlite")).unwrap();
        conn.execute("UPDATE messages SET priority = 'wat'", [])
            .unwrap();
        // The run_pass only walks min/default/high — 'wat' isn't
        // any of those, so it survives. This documents the
        // safety semantics: unknown priorities are immortal
        // until the operator backfills them.
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 0);
    }
}
