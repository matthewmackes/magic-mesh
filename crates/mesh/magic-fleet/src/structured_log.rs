//! OBS-5 (W15) — mesh-replicated structured logging.
//!
//! Each node appends structured log records to its **own** file on the
//! replicated volume (`<workgroup_root>/logs/<host>.jsonl`, own-row
//! authority — the FPG-2 pattern), so the whole fleet's logs converge
//! without a central collector. A record is a JSON object per line
//! (append-only, junk-tolerant on read), carrying the timestamp, host,
//! level, target, message, and arbitrary string fields.
//!
//! This is the engine: the append + the cross-host [`search`] (the
//! Controller-side query PLANES-14's Fleet-logs-search panel renders).
//! Nothing here shells out, so it's fully unit-tested.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One structured log record (one JSON line in a host's log file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Unix epoch milliseconds.
    pub ts_ms: u64,
    /// Originating node's hostname.
    pub host: String,
    /// `error` | `warn` | `info` | `debug` | `trace`.
    pub level: String,
    /// Emitting module/target (e.g. `mackesd::nebula_supervisor`).
    #[serde(default)]
    pub target: String,
    /// The log message.
    pub message: String,
    /// Structured key/value fields.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// The replicated logs directory.
#[must_use]
pub fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}

/// This host's append-only log file.
#[must_use]
pub fn host_log_path(root: &Path, host: &str) -> PathBuf {
    logs_dir(root).join(format!("{host}.jsonl"))
}

// Replicated log leaves are peer-provided input. Keep enough room for a
// useful fleet log while bounding the bytes handed to the JSON parser.
const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;

/// Read one replicated log through a descriptor without following its final
/// symlink, accepting only regular files and no more than `max_bytes`.
///
/// The descriptor metadata check rejects directories and other special files
/// before their contents are consumed. Reading one byte beyond the bound
/// closes the race where a peer grows a regular file after the first metadata
/// snapshot, while invalid UTF-8 is rejected by [`read_bounded_text`] before
/// any JSON parser sees the input.
fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        // Keep unsupported Unix targets fail-closed for a symlink leaf even
        // when their standard library does not expose an O_NOFOLLOW value.
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
            return None;
        }
    }

    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }

    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let max_bytes_u64 = u64::try_from(max_bytes).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes_u64 {
        return None;
    }

    let capacity = usize::try_from(metadata.len())
        .ok()?
        .min(max_bytes)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= max_bytes).then_some(bytes)
}

fn read_bounded_text(path: &Path, max_bytes: usize) -> Option<String> {
    String::from_utf8(read_bounded_regular_file(path, max_bytes)?).ok()
}

/// Append a record to its host's log file (own-row authority — a node
/// only ever writes its own `<host>.jsonl`). One JSON object per line.
///
/// # Errors
/// IO / serialization failures.
pub fn append(root: &Path, record: &LogRecord) -> io::Result<()> {
    let dir = logs_dir(root);
    std::fs::create_dir_all(&dir)?;
    let line = serde_json::to_string(record).map_err(io::Error::other)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(host_log_path(root, &record.host))?;
    writeln!(f, "{line}")
}

/// Read every record from one host's log file (junk-tolerant — an
/// unparseable line is skipped). Order preserved (append order).
#[must_use]
pub fn read_host(root: &Path, host: &str) -> Vec<LogRecord> {
    read_bounded_text(&host_log_path(root, host), MAX_LOG_BYTES)
        .map(|raw| {
            raw.lines()
                .filter_map(|l| serde_json::from_str::<LogRecord>(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// A fleet-wide log query (all filters optional; an unset filter matches
/// everything). The Controller-side search (W15) PLANES-14 drives.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Minimum severity to include (errors-and-up, etc.). `None` = all
    /// levels. Severity rank: error>warn>info>debug>trace.
    pub min_level: Option<String>,
    /// Restrict to one host. `None` = all hosts.
    pub host: Option<String>,
    /// Only records at or after this Unix-ms time.
    pub since_ms: Option<u64>,
    /// Case-insensitive substring the message OR target must contain.
    pub contains: Option<String>,
    /// Cap on returned rows (after sorting newest-first). `None` = no cap.
    pub limit: Option<usize>,
}

/// Severity rank — higher is more severe. Unknown levels rank as `info`.
#[must_use]
fn level_rank(level: &str) -> u8 {
    match level.to_ascii_lowercase().as_str() {
        "error" => 4,
        "warn" | "warning" => 3,
        "debug" => 1,
        "trace" => 0,
        _ => 2,
    }
}

/// Search the whole fleet's logs (every `<host>.jsonl`) against `query`,
/// returning matching records **newest-first**, capped by `query.limit`.
#[must_use]
pub fn search(root: &Path, query: &LogQuery) -> Vec<LogRecord> {
    let Ok(entries) = std::fs::read_dir(logs_dir(root)) else {
        return Vec::new();
    };
    let min_rank = query.min_level.as_deref().map(level_rank);
    let needle = query.contains.as_ref().map(|s| s.to_ascii_lowercase());

    let mut out: Vec<LogRecord> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .filter(|host| query.host.as_ref().is_none_or(|h| h == host))
        .flat_map(|host| read_host(root, &host))
        .filter(|r| min_rank.is_none_or(|m| level_rank(&r.level) >= m))
        .filter(|r| query.since_ms.is_none_or(|s| r.ts_ms >= s))
        .filter(|r| {
            needle.as_ref().is_none_or(|n| {
                r.message.to_ascii_lowercase().contains(n)
                    || r.target.to_ascii_lowercase().contains(n)
            })
        })
        .collect();
    out.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms)); // newest first
    if let Some(limit) = query.limit {
        out.truncate(limit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(host: &str, ts: u64, level: &str, target: &str, msg: &str) -> LogRecord {
        LogRecord {
            ts_ms: ts,
            host: host.into(),
            level: level.into(),
            target: target.into(),
            message: msg.into(),
            fields: BTreeMap::new(),
        }
    }

    #[test]
    fn append_and_read_host_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        append(
            tmp.path(),
            &rec("pine", 100, "info", "mackesd::a", "started"),
        )
        .unwrap();
        append(
            tmp.path(),
            &rec("pine", 200, "warn", "mackesd::b", "slow tick"),
        )
        .unwrap();
        let got = read_host(tmp.path(), "pine");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].message, "slow tick");
    }

    #[test]
    fn search_across_hosts_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        append(tmp.path(), &rec("pine", 100, "info", "t", "a")).unwrap();
        append(tmp.path(), &rec("oak", 300, "error", "t", "boom")).unwrap();
        append(tmp.path(), &rec("pine", 200, "info", "t", "b")).unwrap();
        let all = search(tmp.path(), &LogQuery::default());
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts_ms, 300, "newest first");
        assert_eq!(all[2].ts_ms, 100);
    }

    #[test]
    fn search_filters_level_host_since_contains_and_limit() {
        let tmp = tempfile::tempdir().unwrap();
        append(tmp.path(), &rec("pine", 100, "info", "net", "hello")).unwrap();
        append(tmp.path(), &rec("pine", 200, "error", "net", "disk FULL")).unwrap();
        append(tmp.path(), &rec("oak", 300, "warn", "fw", "zone set")).unwrap();

        // min_level=warn drops the info row.
        let warns = search(
            tmp.path(),
            &LogQuery {
                min_level: Some("warn".into()),
                ..Default::default()
            },
        );
        assert_eq!(warns.len(), 2);

        // host filter.
        let pine = search(
            tmp.path(),
            &LogQuery {
                host: Some("pine".into()),
                ..Default::default()
            },
        );
        assert_eq!(pine.len(), 2);

        // since_ms.
        let recent = search(
            tmp.path(),
            &LogQuery {
                since_ms: Some(250),
                ..Default::default()
            },
        );
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].host, "oak");

        // contains (case-insensitive, message OR target).
        let full = search(
            tmp.path(),
            &LogQuery {
                contains: Some("full".into()),
                ..Default::default()
            },
        );
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].message, "disk FULL");

        // limit caps newest-first.
        let one = search(
            tmp.path(),
            &LogQuery {
                limit: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].ts_ms, 300);
    }

    #[test]
    fn missing_logs_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(search(tmp.path(), &LogQuery::default()).is_empty());
        assert!(read_host(tmp.path(), "ghost").is_empty());
    }

    #[test]
    fn junk_lines_are_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = logs_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pine.jsonl"),
            "not json\n{\"ts_ms\":1,\"host\":\"pine\",\"level\":\"info\",\"message\":\"ok\"}\n",
        )
        .unwrap();
        let got = read_host(tmp.path(), "pine");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "ok");
    }

    #[test]
    fn nonregular_log_leaf_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = logs_dir(tmp.path());
        std::fs::create_dir_all(dir.join("pine.jsonl")).unwrap();
        assert!(read_host(tmp.path(), "pine").is_empty());
    }

    #[test]
    fn oversized_log_leaf_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = logs_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pine.jsonl"), vec![b'x'; MAX_LOG_BYTES + 1]).unwrap();
        assert!(read_host(tmp.path(), "pine").is_empty());
    }

    #[test]
    fn invalid_utf8_log_leaf_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = logs_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pine.jsonl"), [0xff, 0xfe]).unwrap();
        assert!(read_host(tmp.path(), "pine").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_log_leaf_is_skipped() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = logs_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("real.jsonl");
        std::fs::write(
            &target,
            br#"{"ts_ms":1,"host":"pine","level":"info","message":"ok"}
"#,
        )
        .unwrap();
        symlink(&target, dir.join("pine.jsonl")).unwrap();
        assert!(read_host(tmp.path(), "pine").is_empty());
    }
}
