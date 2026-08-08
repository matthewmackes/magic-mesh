//! PD-11 compatibility result storage for the retired service-lifecycle lane.
//!
//! VM/container mutations now use the authenticated typed Workload authority.
//! The old replicated request writer, command planner, and `lifecycle_exec`
//! actuator are gone; this narrow file-backed result reader remains only so
//! already-issued legacy result polls can fail or complete honestly during
//! migration.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bound peer-controlled result materialization before deserialization.
const MAX_LIFECYCLE_RECORD_BYTES: usize = 256 * 1024;

/// A result for one already-issued legacy lifecycle request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleResult {
    /// Request id that this result answers.
    pub id: String,
    /// `true` if the operation succeeded; `false` if the retired executor caught an error.
    pub ok: bool,
    /// Human-readable error message when `ok` is `false`; empty on success.
    #[serde(default)]
    pub error: String,
}

/// Validate one request-controlled filename component before it reaches the
/// replicated root. Lifecycle ids and hosts are keys, never paths.
fn safe_component<'a>(field: &str, value: &'a str) -> io::Result<&'a str> {
    if value.is_empty()
        || value.trim() != value
        || value == "."
        || value == ".."
        || value.len() > 255
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{field}` must be one path-safe [A-Za-z0-9._-] segment"),
        ));
    }
    Ok(value)
}

/// Resolve the per-target result directory after validating the target as one
/// ordinary component. An absolute/traversing host can never replace the root.
fn lifecycle_dir(workgroup_root: &Path, target_host: &str) -> io::Result<PathBuf> {
    Ok(workgroup_root
        .join("fleet")
        .join("lifecycle")
        .join(safe_component("target host", target_host)?))
}

/// Read one replicated lifecycle result through the descriptor that will be
/// consumed. Reject final symlinks, blocking special files, oversized input,
/// and growth beyond the bound before deserialization.
fn read_bounded_lifecycle_record(path: &Path) -> Option<String> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_LIFECYCLE_RECORD_BYTES as u64 {
        return None;
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_LIFECYCLE_RECORD_BYTES)
            .min(MAX_LIFECYCLE_RECORD_BYTES)
            .saturating_add(1),
    );
    file.take((MAX_LIFECYCLE_RECORD_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_LIFECYCLE_RECORD_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Write the result for request `id` on `target_host`'s dir.
///
/// # Errors
/// IO/serialization failures.
pub fn write_result(
    workgroup_root: &Path,
    target_host: &str,
    result: &LifecycleResult,
) -> io::Result<PathBuf> {
    safe_component("request id", &result.id)?;
    let dir = lifecycle_dir(workgroup_root, target_host)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.result.json", result.id));
    let tmp = dir.join(format!(".{}.result.tmp", result.id));
    std::fs::write(&tmp, serde_json::to_string_pretty(result)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read (and consume) the result for `id`, if present yet.
#[must_use]
pub fn take_result(workgroup_root: &Path, target_host: &str, id: &str) -> Option<LifecycleResult> {
    safe_component("request id", id).ok()?;
    let path = lifecycle_dir(workgroup_root, target_host)
        .ok()?
        .join(format!("{id}.result.json"));
    let raw = read_bounded_lifecycle_record(&path)?;
    let result = serde_json::from_str(&raw).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_round_trip_and_consume() {
        let tmp = tempfile::tempdir().unwrap();
        write_result(
            tmp.path(),
            "oak",
            &LifecycleResult {
                id: "r1".into(),
                ok: false,
                error: "no such container".into(),
            },
        )
        .unwrap();
        let result = take_result(tmp.path(), "oak", "r1").unwrap();
        assert!(!result.ok);
        assert_eq!(result.error, "no such container");
        assert!(take_result(tmp.path(), "oak", "r1").is_none(), "consumed");
    }

    #[test]
    fn result_paths_reject_every_escape_before_io() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let absolute = outside.to_string_lossy();
        let result = LifecycleResult {
            id: "01SAFE".into(),
            ok: true,
            error: String::new(),
        };

        for host in [absolute.as_ref(), "../outside", "a/b", "", ".", ".."] {
            assert!(write_result(tmp.path(), host, &result).is_err());
        }
        for id in ["../escape", "a/b", "", ".", ".."] {
            let invalid = LifecycleResult {
                id: id.into(),
                ..result.clone()
            };
            assert!(write_result(tmp.path(), "oak", &invalid).is_err());
            assert!(take_result(tmp.path(), "oak", id).is_none());
        }
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn result_reader_rejects_final_symlinks_without_consuming_targets() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fleet/lifecycle/oak");
        std::fs::create_dir_all(&dir).unwrap();
        let target = tmp.path().join("outside-result.json");
        std::fs::write(
            &target,
            serde_json::to_string(&LifecycleResult {
                id: "linked-result".into(),
                ok: true,
                error: String::new(),
            })
            .unwrap(),
        )
        .unwrap();
        symlink(&target, dir.join("linked-result.result.json")).unwrap();

        assert!(take_result(tmp.path(), "oak", "linked-result").is_none());
        assert!(target.exists());
    }

    #[test]
    fn result_reader_leaves_oversized_payloads_unconsumed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fleet/lifecycle/oak");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large.result.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"id":"large","ok":true,"error":"{}"}}"#,
                "x".repeat(MAX_LIFECYCLE_RECORD_BYTES)
            ),
        )
        .unwrap();

        assert!(take_result(tmp.path(), "oak", "large").is_none());
        assert!(path.exists(), "invalid replicated input is not consumed");
    }
}
