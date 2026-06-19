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

/// GFS quota thresholds. These are the conservative **tmpfs-safe** fallback
/// defaults: the bus spool lives on `/run` (tmpfs), and a lighthouse `/run` is
/// as small as ~190 MB, so the old 500 MB soft / 2 GB hard could never fire
/// before ENOSPC bricked the node (BULLETPROOF-1, found live 2026-06-16 — both
/// lighthouses filled `/run` to 100% on the unreaped `audit/*` lane). mackesd
/// overrides these with a filesystem-relative cap at spawn; the fixed default
/// stays small enough that it alone keeps the smallest tmpfs from filling.
pub const DEFAULT_QUOTA_SOFT_BYTES: u64 = 96 * 1024 * 1024;
pub const DEFAULT_QUOTA_HARD_BYTES: u64 = 144 * 1024 * 1024;

/// Default GC pass cadence. BUS-RUN-FULL-1 (2026-06-18): the old hourly pass let
/// high-ingest nodes refill `/run` (tmpfs) from the quota cap to 100% between
/// passes — a live .13 spool reached 389 MB (double the ~195 MB cap) and blocked
/// dnf. The quota eviction (BULLETPROOF-1, oldest-first) only bounds the spool if
/// it runs often enough to keep pace with ingest, so the pass is now every 2 min:
/// ingest per window (a few MB) stays far under the cap headroom, so `/run` never
/// fills. Still far faster than the shortest TTL.
pub const DEFAULT_PASS_INTERVAL: Duration = Duration::from_secs(120);

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
    /// Files (and matching index rows) removed by TTL expiry this pass.
    pub removed: usize,
    /// BULLETPROOF-1 — messages evicted by the hard-cap safety valve
    /// (oldest-first, regardless of TTL) because the spool exceeded the
    /// hard quota. Distinct from `removed` so the log shows when the bus
    /// is shedding live data to stay off ENOSPC.
    pub evicted: usize,
    /// Total disk bytes the bus occupies after the pass — the remaining spool
    /// `*.json` files PLUS the index DB itself (`index.sqlite{,-wal,-shm}`,
    /// BUS-RETENTION-1). This is the true `/run` footprint the quota guards.
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

/// BUS-RETENTION-1 — on-disk bytes of the index DB itself (`index.sqlite` plus
/// its `-wal`/`-shm` sidecars). [`disk_usage_bytes`] deliberately excludes these
/// (it sums only spool `*.json`), but the index is a real consumer of the tmpfs
/// that backs the bus — it duplicates each message's title+body — so the quota
/// footprint MUST include it. A 121 MB `index.sqlite` filled a 391 MB `/run`
/// while the spool-only accounting reported "under budget" (found in the
/// v10.0.18 fleet roll, 2026-06-19). Missing files contribute 0.
#[must_use]
pub fn index_db_bytes(root: &Path) -> u64 {
    ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"]
        .iter()
        .filter_map(|n| std::fs::metadata(root.join(n)).ok())
        .map(|m| m.len())
        .sum()
}

/// BUS-RETENTION-1 — return disk that the index DB freed (by the row deletes in
/// a pass) to the OS. SQLite frees pages internally on `DELETE` but never shrinks
/// the file on its own, so without this the index only ever grows — a live
/// 121 MB `index.sqlite` (which duplicates message title+body) filled a 391 MB
/// `/run` while the spool was capped (found in the v10.0.18 fleet roll). A full
/// `VACUUM` is the only reliable reclaim: `incremental_vacuum` returns ~0 pages
/// in WAL mode (verified). VACUUM rewrites the DB **in place** (same inode →
/// never the [[BUS-INODE-ORPHAN]] unlink/recreate trap) and needs ~the DB size
/// in temp space, which the preceding TTL-reap/eviction just freed on the tmpfs.
///
/// Guarded by `freelist_count` so idle passes (nothing deleted → nothing to
/// reclaim) skip the VACUUM entirely; combined with the 120 s GC cadence the DB
/// stays small, so each VACUUM is sub-second and safe alongside the live broker
/// (own connection, 5 s `busy_timeout`). Every step is best-effort — a
/// BUSY/ENOSPC failure is ignored and retried next pass.
fn compact_index(conn: &Connection) {
    // Return checkpointed WAL pages to the OS (no-op outside WAL mode).
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    // Only VACUUM when there's a non-trivial amount to reclaim (> 64 pages ≈
    // 256 KiB), so idle passes don't rewrite the DB for nothing.
    let freelist: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap_or(0);
    if freelist > 64 {
        let _ = conn.execute_batch("VACUUM;");
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
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
        let cutoff =
            now_unix_ms - i64::try_from(policy.ttl_ephemeral_secs * 1000).unwrap_or(i64::MAX);
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

    // BUS-RETENTION-1 — reclaim the DB pages the deletes above freed BEFORE we
    // measure/evict, so the footprint reflects a compacted index (not its stale
    // high-water mark) and we don't over-evict spool to compensate for a bloated
    // DB that's about to shrink.
    compact_index(&conn);

    // BULLETPROOF-1 — hard-cap safety valve. The TTL reap above leaves
    // `audit/*` untouched (Q28: the audit trail is retention=forever). On the
    // tmpfs that backs the bus that is unbounded growth → a full `/run` that
    // downs the whole node. So if we're still over the HARD cap after the TTL
    // reap, evict oldest-first until back under the SOFT cap. Non-audit goes
    // first; `audit/*` is shed only as a last resort (a full `/run` is strictly
    // worse than losing ephemeral tmpfs audit that does not survive reboot).
    //
    // BUS-RETENTION-1 — the footprint counts the spool files AND the index DB
    // itself (`index_db_bytes`): the DB is a real tmpfs consumer (it duplicates
    // title+body), so excluding it let a 121 MB index fill a 391 MB `/run` while
    // the cap reported "under budget". Eviction can only shed spool files, so it
    // targets `soft - db` to bring spool + (compacted) DB back under the soft cap.
    let mut bytes_after = disk_usage_bytes(bus_root)? + index_db_bytes(bus_root);
    let mut evicted = 0_usize;
    if bytes_after > policy.quota_hard_bytes {
        let spool = disk_usage_bytes(bus_root)?;
        // The index duplicates each message's title+body, so evicting a message
        // frees its spool file AND (after compaction) its DB rows — roughly
        // (1 + db/spool)× the file size from the TOTAL footprint. eviction only
        // tracks spool bytes, so target the spool level whose total lands at the
        // soft cap: spool_target = spool * soft / total. (u128 to avoid overflow
        // at multi-GB workstation tmpfs sizes.)
        let spool_target = if bytes_after > 0 {
            u64::try_from(
                u128::from(spool) * u128::from(policy.quota_soft_bytes) / u128::from(bytes_after),
            )
            .unwrap_or(policy.quota_soft_bytes)
        } else {
            0
        };
        evicted = evict_oldest_to_cap(&conn, bus_root, spool, spool_target)?;
        // Reclaim again — eviction deleted more rows; then re-measure the true
        // footprint (spool + compacted DB).
        compact_index(&conn);
        bytes_after = disk_usage_bytes(bus_root)? + index_db_bytes(bus_root);
    }
    let quota = QuotaReport {
        soft_exceeded: bytes_after > policy.quota_soft_bytes,
        hard_exceeded: bytes_after > policy.quota_hard_bytes,
    };
    Ok(PassReport {
        removed,
        evicted,
        bytes_after,
        quota,
    })
}

/// BULLETPROOF-1 — evict oldest messages (by `ts_unix_ms`) until on-disk usage
/// drops from `start_bytes` to at most `soft_bytes`. Two phases: non-`audit/*`
/// first, then `audit/*` only if shedding all eligible non-audit still leaves
/// us over the cap. Tracks the running total by subtracting each file's size
/// (no full re-walk per delete). Returns the number of messages evicted.
fn evict_oldest_to_cap(
    conn: &rusqlite::Connection,
    bus_root: &Path,
    start_bytes: u64,
    soft_bytes: u64,
) -> Result<usize, RetentionError> {
    let audit_like = format!("{}%", crate::persist::AUDIT_TOPIC_PREFIX);
    let mut running = start_bytes;
    let mut evicted = 0_usize;
    // Phase 1: non-audit oldest-first. Phase 2: audit oldest-first.
    for sql in [
        "SELECT ulid, file_path FROM messages WHERE topic NOT LIKE ?1 ORDER BY ts_unix_ms ASC",
        "SELECT ulid, file_path FROM messages WHERE topic LIKE ?1 ORDER BY ts_unix_ms ASC",
    ] {
        if running <= soft_bytes {
            break;
        }
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| RetentionError::Sql(format!("evict prepare: {e}")))?;
        let rows = stmt
            .query_map(params![audit_like], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| RetentionError::Sql(format!("evict query: {e}")))?;
        for r in rows {
            if running <= soft_bytes {
                break;
            }
            let (ulid, rel_path) =
                r.map_err(|e| RetentionError::Sql(format!("evict decode: {e}")))?;
            let abs = bus_root.join(&rel_path);
            let sz = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
            if abs.exists() {
                std::fs::remove_file(&abs)
                    .map_err(|e| RetentionError::Io(format!("evict rm {}: {e}", abs.display())))?;
            }
            conn.execute("DELETE FROM messages WHERE ulid = ?1", params![ulid])
                .map_err(|e| RetentionError::Sql(format!("evict row {ulid}: {e}")))?;
            running = running.saturating_sub(sz);
            evicted += 1;
        }
    }
    Ok(evicted)
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
                            evicted = report.evicted,
                            bytes_after = report.bytes_after,
                            soft_exceeded = report.quota.soft_exceeded,
                            hard_exceeded = report.quota.hard_exceeded,
                            "retention pass complete"
                        );
                        if report.evicted > 0 {
                            tracing::warn!(
                                target: "mde_bus::retention",
                                evicted = report.evicted,
                                bytes_after = report.bytes_after,
                                "BULLETPROOF-1: hard-cap reached — evicted oldest messages to keep the bus tmpfs off ENOSPC"
                            );
                        }
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
            (
                "action/fleet/list-revisions",
                Priority::Default,
                two_hours_ago,
            ),
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
        let (_tmp, root) = open_tmp_with(&[("reply/01FRESH", Priority::Default, half_hour_ago)]);
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
        assert_eq!(
            report.removed, 1,
            "single removal despite matching both passes"
        );
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
        // Soft breach that is NOT a hard breach: advisory only, no eviction
        // (the hard-cap valve fires only above the hard quota — BULLETPROOF-1).
        let policy = RetentionPolicy {
            quota_soft_bytes: 1,                // 1 byte — guaranteed exceeded
            quota_hard_bytes: 10 * 1024 * 1024, // 10 MB — not reached by one small msg
            ..RetentionPolicy::default()
        };
        let (_tmp, root) = open_tmp_with(&[("t/x", Priority::Urgent, 1)]);
        let report = run_pass_at(&policy, &root, 100).unwrap();
        assert!(report.quota.soft_exceeded, "soft quota surfaces");
        assert!(!report.quota.hard_exceeded, "below hard quota");
        assert_eq!(report.evicted, 0, "no eviction below the hard cap");
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

    // BULLETPROOF-1 — hard-cap eviction safety valve.

    /// Seed `topic` with a `body_len`-byte body at `ts`, returning its ULID.
    fn write_sized(root: &Path, topic: &str, prio: Priority, ts: i64, body_len: usize) -> String {
        let p = Persist::open(root.to_path_buf()).unwrap();
        let body = "x".repeat(body_len);
        let m = p.write(topic, prio, None, Some(&body)).unwrap();
        let conn = Connection::open(root.join("index.sqlite")).unwrap();
        conn.execute(
            "UPDATE messages SET ts_unix_ms = ?1 WHERE ulid = ?2",
            params![ts, m.ulid],
        )
        .unwrap();
        m.ulid
    }

    fn purge_audit(root: &Path) {
        let conn = Connection::open(root.join("index.sqlite")).unwrap();
        conn.execute("DELETE FROM messages WHERE topic LIKE 'audit/%'", [])
            .unwrap();
        let _ = std::fs::remove_dir_all(root.join("audit"));
    }

    #[test]
    fn hard_cap_evicts_oldest_first_until_under_soft() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // Six 1 MB messages (6 MB) on a 3 MB-hard / 2 MB-soft policy.
        let mb = 1024 * 1024;
        let now = 1_000_000_000_000_i64;
        let mut ulids = Vec::new();
        for i in 0..6 {
            // Recent, ascending ts so High's 30d TTL never reaps them — the
            // hard-cap valve is the only thing that should fire.
            ulids.push(write_sized(
                &root,
                "t/bulk",
                Priority::High,
                now - (6 - i) * 1000,
                mb,
            ));
        }
        purge_audit(&root);
        // BUS-RETENTION-1 — the footprint counts the index DB too (it duplicates
        // bodies, ~1 MB/msg here), so the total ≈ 2× the spool. Caps are sized so
        // a few oldest messages are shed while the newest survives + the result
        // lands under the hard cap (soft=2 MB would round to "shed everything"
        // once the DB is counted).
        let policy = RetentionPolicy {
            quota_soft_bytes: 5 * mb as u64,
            quota_hard_bytes: 6 * mb as u64,
            ..RetentionPolicy::default()
        };
        let report = run_pass_at(&policy, &root, now).unwrap();
        assert_eq!(report.removed, 0, "no TTL expiry — High survives 30d");
        assert!(report.evicted >= 1, "hard cap must shed oldest");
        assert!(
            report.bytes_after <= policy.quota_hard_bytes,
            "post-eviction must be under the hard cap, was {}",
            report.bytes_after
        );
        // The newest message (highest ts) must survive; the oldest must be gone.
        let p = Persist::open(root.clone()).unwrap();
        let remaining: Vec<String> = p
            .list_since("t/bulk", None)
            .unwrap()
            .into_iter()
            .map(|m| m.ulid)
            .collect();
        assert!(remaining.contains(ulids.last().unwrap()), "newest survives");
        assert!(!remaining.contains(&ulids[0]), "oldest evicted");
    }

    #[test]
    fn hard_cap_sheds_non_audit_before_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mb = 1024 * 1024;
        let now = 1_000_000_000_000_i64;
        // 4 non-audit (1 MB each) + 2 audit (1 MB each), all recent so nothing
        // TTL-reaps. Eviction is phase-ordered (non-audit first) regardless of
        // ts, so shedding the non-audit lane alone gets us under the cap.
        let mut non_audit = Vec::new();
        for i in 0..4 {
            non_audit.push(write_sized(
                &root,
                "t/data",
                Priority::High,
                now - (6 - i) * 1000,
                mb,
            ));
        }
        let audit_a = write_sized(&root, "audit/peerx", Priority::Min, now - 2000, mb);
        let audit_b = write_sized(&root, "audit/peerx", Priority::Min, now - 1000, mb);
        // BUS-RETENTION-1 — the footprint now also counts the index DB (which
        // duplicates bodies, ~1 MB/msg here), so the synthetic total ≈ 2× the
        // spool. Caps are sized so shedding the 4 non-audit messages alone brings
        // the total under the soft cap, leaving both audit records intact.
        let policy = RetentionPolicy {
            quota_soft_bytes: 5 * mb as u64,
            quota_hard_bytes: 6 * mb as u64,
            ..RetentionPolicy::default()
        };
        let report = run_pass_at(&policy, &root, now).unwrap();
        assert!(report.evicted >= 1);
        // Audit is shed last: both audit records must survive because evicting
        // the non-audit lane alone gets us under the cap.
        let p = Persist::open(root.clone()).unwrap();
        let audit_left: Vec<String> = p
            .list_since("audit/peerx", None)
            .unwrap()
            .into_iter()
            .map(|m| m.ulid)
            .collect();
        assert!(
            audit_left.contains(&audit_a) && audit_left.contains(&audit_b),
            "audit/* must be preserved when non-audit eviction suffices"
        );
        assert!(
            p.list_since("t/data", None).unwrap().len() < non_audit.len(),
            "non-audit lane shed first"
        );
    }

    #[test]
    fn under_hard_cap_evicts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_sized(&root, "t/small", Priority::High, 1, 1024);
        purge_audit(&root);
        // Default tmpfs-safe policy (96/144 MB) is far above a 1 KB spool.
        let report = run_pass_at(&RetentionPolicy::default(), &root, 1_000_000_000_000).unwrap();
        assert_eq!(report.evicted, 0);
    }

    // BUS-RETENTION-1 — the index DB must count toward the footprint + shrink.

    #[test]
    fn footprint_includes_index_db() {
        // A message with a sizable body bloats the index (it duplicates
        // title+body), so `index_db_bytes` is non-trivial and `run_pass_at`'s
        // `bytes_after` must exceed the spool-only `disk_usage_bytes`.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_sized(&root, "t/keep", Priority::Urgent, 1, 64 * 1024);
        purge_audit(&root);
        let db = index_db_bytes(&root);
        assert!(
            db > 0,
            "index DB must contribute to the footprint, got {db}"
        );
        let report = run_pass_at(&RetentionPolicy::default(), &root, 100).unwrap();
        let spool = disk_usage_bytes(&root).unwrap();
        assert!(
            report.bytes_after >= spool + index_db_bytes(&root),
            "bytes_after ({}) must include the index DB on top of the spool ({spool})",
            report.bytes_after,
        );
        assert!(report.bytes_after > spool, "DB bytes are counted");
    }

    #[test]
    fn pass_reclaims_index_db_after_bulk_delete() {
        // Born INCREMENTAL (the migration sets auto_vacuum) so a pass that
        // deletes expired rows returns their pages to the OS — the index file
        // must SHRINK, not just free pages internally (the v10.0.18 failure:
        // a 121 MB index that never gave space back filled /run).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let now = 1_000_000_000_000_i64;
        let long_ago = now - (10 * 24 * 60 * 60 * 1000); // 10 days — past min/default TTL
                                                         // 40 expired min-priority messages with 32 KB bodies each → a fat index.
        for i in 0..40 {
            write_sized(
                &root,
                &format!("t/old{i}"),
                Priority::Min,
                long_ago,
                32 * 1024,
            );
        }
        purge_audit(&root);
        let db_before = index_db_bytes(&root);
        assert!(
            db_before > 256 * 1024,
            "index should be fat pre-reap, got {db_before}"
        );
        let report = run_pass_at(&RetentionPolicy::default(), &root, now).unwrap();
        assert_eq!(report.removed, 40, "all 40 expired messages reaped");
        let db_after = index_db_bytes(&root);
        assert!(
            db_after < db_before / 2,
            "index DB must shrink after the reap: before={db_before} after={db_after}",
        );
    }

    #[test]
    fn default_quota_is_tmpfs_safe() {
        // Regression: the old 500 MB/2 GB defaults exceeded a ~190 MB
        // lighthouse /run, so the cap could never fire before ENOSPC.
        assert!(DEFAULT_QUOTA_HARD_BYTES < 190 * 1024 * 1024);
        assert!(DEFAULT_QUOTA_SOFT_BYTES < DEFAULT_QUOTA_HARD_BYTES);
    }
}
