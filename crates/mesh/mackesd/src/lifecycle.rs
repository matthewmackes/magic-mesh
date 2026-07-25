//! PD-11 (L9/L16) — remote service lifecycle over the replicated
//! volume.
//!
//! "Start/stop/restart that container/VM on that peer": the GUI's
//! local mackesd writes a request file under
//! `<root>/fleet/lifecycle/<target-host>/<id>.json`; replication
//! carries it; the target's `lifecycle_exec` worker consumes it,
//! **validates the name against what its own local probe actually
//! offers** (never arbitrary `podman`/`virsh` argument passthrough —
//! the design-doc rail), executes, and writes
//! `<id>.result.json` back for the requester to poll. Files, not
//! sockets — the same no-fixed-center transport as nudges/acks.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Lifecycle requests and results are compact replicated control records.
/// Bound their materialization before `serde_json` sees peer-controlled bytes.
const MAX_LIFECYCLE_RECORD_BYTES: usize = 256 * 1024;

/// A lifecycle request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleRequest {
    /// Request id (unique per request; the result file is named by it).
    pub id: String,
    /// `container` | `vm`.
    pub kind: String,
    /// The container/guest name — must be present in the target's
    /// own probe at execution time.
    pub name: String,
    /// `start` | `stop` | `restart`.
    pub op: String,
    /// Requesting node (advisory, for the audit trail).
    #[serde(default)]
    pub from: String,
}

/// A lifecycle result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleResult {
    /// Request id — matches the [`LifecycleRequest::id`] this result answers.
    pub id: String,
    /// `true` if the operation succeeded; `false` if the executor caught an error.
    pub ok: bool,
    /// Human-readable error message when `ok` is `false`; empty on success.
    #[serde(default)]
    pub error: String,
}

/// Validate one request-controlled filename component before it reaches the
/// replicated root. Lifecycle ids, hosts, and offered service names are keys,
/// never paths.
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

/// Resolve the per-target request directory after validating the target as one
/// ordinary component. An absolute/traversing host can never replace the root.
fn lifecycle_dir(workgroup_root: &Path, target_host: &str) -> io::Result<PathBuf> {
    Ok(workgroup_root
        .join("fleet")
        .join("lifecycle")
        .join(safe_component("target host", target_host)?))
}

/// Read one replicated lifecycle record through the descriptor that will be
/// consumed. Reject final symlinks, blocking special files, oversized input,
/// and growth beyond the bound before deserialization.
fn read_bounded_lifecycle_record(path: &Path) -> Option<String> {
    use std::io::Read as _;

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

/// `true` for the op vocabulary the executor accepts.
#[must_use]
pub fn valid_op(op: &str) -> bool {
    matches!(op, "start" | "stop" | "restart")
}

/// `true` for the kind vocabulary the executor accepts.
#[must_use]
pub fn valid_kind(kind: &str) -> bool {
    matches!(kind, "container" | "vm")
}

/// Write a request for `target_host` (atomic temp + rename).
///
/// # Errors
/// IO/serialization failures, or an invalid kind/op.
pub fn write_request(
    workgroup_root: &Path,
    target_host: &str,
    req: &LifecycleRequest,
) -> io::Result<PathBuf> {
    if !valid_kind(&req.kind) || !valid_op(&req.op) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid kind/op: {}/{}", req.kind, req.op),
        ));
    }
    safe_component("request id", &req.id)?;
    safe_component("service name", &req.name)?;
    let dir = lifecycle_dir(workgroup_root, target_host)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", req.id));
    let tmp = dir.join(format!(".{}.json.tmp", req.id));
    std::fs::write(&tmp, serde_json::to_string_pretty(req)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Consume (read + delete) every pending request addressed to
/// `self_host`. Result files (`*.result.json`) are skipped.
#[must_use]
pub fn take_requests(workgroup_root: &Path, self_host: &str) -> Vec<LifecycleRequest> {
    let Ok(dir) = lifecycle_dir(workgroup_root, self_host) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !name.ends_with(".json") || name.ends_with(".result.json") || name.starts_with('.') {
            continue;
        }
        if let Some(raw) = read_bounded_lifecycle_record(&p) {
            if let Ok(req) = serde_json::from_str::<LifecycleRequest>(&raw) {
                let _ = std::fs::remove_file(&p);
                if valid_kind(&req.kind)
                    && valid_op(&req.op)
                    && safe_component("request id", &req.id).is_ok()
                    && safe_component("service name", &req.name).is_ok()
                {
                    out.push(req);
                }
            }
        }
    }
    out
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

/// The executor's command plan for a validated request (pure — the
/// worker spawns it). `None` for vocabulary violations.
#[must_use]
pub fn command_plan(req: &LifecycleRequest) -> Option<(&'static str, Vec<String>)> {
    if !valid_kind(&req.kind) || !valid_op(&req.op) {
        return None;
    }
    match req.kind.as_str() {
        "container" => Some(("podman", vec![req.op.clone(), req.name.clone()])),
        "vm" => {
            let verb = match req.op.as_str() {
                "start" => "start",
                "stop" => "shutdown",
                _ => "reboot",
            };
            Some(("virsh", vec![verb.to_string(), req.name.clone()]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, kind: &str, name: &str, op: &str) -> LifecycleRequest {
        LifecycleRequest {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            op: op.into(),
            from: "peer:test".into(),
        }
    }

    #[test]
    fn request_round_trips_and_consumes_once() {
        let tmp = tempfile::tempdir().unwrap();
        write_request(tmp.path(), "oak", &req("r1", "container", "nginx", "start")).unwrap();
        let got = take_requests(tmp.path(), "oak");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "nginx");
        assert!(take_requests(tmp.path(), "oak").is_empty(), "consumed");
    }

    #[test]
    fn invalid_vocabulary_is_refused_at_write() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(write_request(tmp.path(), "oak", &req("r", "container", "x", "explode")).is_err());
        assert!(write_request(tmp.path(), "oak", &req("r", "kernel", "x", "stop")).is_err());
    }

    #[test]
    fn request_and_result_paths_reject_every_escape_before_io() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let absolute = outside.to_string_lossy();

        for host in [absolute.as_ref(), "../outside", "a/b", "", ".", ".."] {
            assert!(write_request(
                tmp.path(),
                host,
                &req("01SAFE", "container", "nginx", "start")
            )
            .is_err());
            assert!(write_result(
                tmp.path(),
                host,
                &LifecycleResult {
                    id: "01SAFE".into(),
                    ok: true,
                    error: String::new(),
                }
            )
            .is_err());
        }
        for id in ["../escape", "a/b", "", ".", ".."] {
            assert!(
                write_request(tmp.path(), "oak", &req(id, "container", "nginx", "start")).is_err()
            );
            assert!(write_result(
                tmp.path(),
                "oak",
                &LifecycleResult {
                    id: id.into(),
                    ok: true,
                    error: String::new(),
                }
            )
            .is_err());
            assert!(take_result(tmp.path(), "oak", id).is_none());
        }
        assert!(write_request(
            tmp.path(),
            "oak",
            &req("01SAFE", "container", "../nginx", "start")
        )
        .is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

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
        let r = take_result(tmp.path(), "oak", "r1").unwrap();
        assert!(!r.ok);
        assert_eq!(r.error, "no such container");
        assert!(take_result(tmp.path(), "oak", "r1").is_none(), "consumed");
    }

    #[test]
    fn results_are_not_consumed_as_requests() {
        let tmp = tempfile::tempdir().unwrap();
        write_result(
            tmp.path(),
            "oak",
            &LifecycleResult {
                id: "r1".into(),
                ok: true,
                error: String::new(),
            },
        )
        .unwrap();
        assert!(take_requests(tmp.path(), "oak").is_empty());
    }

    #[test]
    fn request_reader_skips_oversized_invalid_and_non_regular_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fleet/lifecycle/oak");
        std::fs::create_dir_all(&dir).unwrap();
        write_request(
            tmp.path(),
            "oak",
            &req("good", "container", "nginx", "start"),
        )
        .unwrap();
        std::fs::write(
            dir.join("oversized.json"),
            vec![b'{'; MAX_LIFECYCLE_RECORD_BYTES + 1],
        )
        .unwrap();
        std::fs::write(dir.join("invalid.json"), [0xff, 0xfe]).unwrap();
        std::fs::create_dir(dir.join("directory.json")).unwrap();

        let got = take_requests(tmp.path(), "oak");
        assert_eq!(got, vec![req("good", "container", "nginx", "start")]);
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_readers_reject_final_symlinks_without_consuming_targets() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fleet/lifecycle/oak");
        std::fs::create_dir_all(&dir).unwrap();

        let request_target = tmp.path().join("outside-request.json");
        std::fs::write(
            &request_target,
            serde_json::to_string(&req("linked", "container", "nginx", "start")).unwrap(),
        )
        .unwrap();
        symlink(&request_target, dir.join("linked.json")).unwrap();
        assert!(take_requests(tmp.path(), "oak").is_empty());
        assert!(request_target.exists());

        let result_target = tmp.path().join("outside-result.json");
        std::fs::write(
            &result_target,
            serde_json::to_string(&LifecycleResult {
                id: "linked-result".into(),
                ok: true,
                error: String::new(),
            })
            .unwrap(),
        )
        .unwrap();
        symlink(&result_target, dir.join("linked-result.result.json")).unwrap();
        assert!(take_result(tmp.path(), "oak", "linked-result").is_none());
        assert!(result_target.exists());
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

    #[test]
    fn command_plans_map_the_vocabulary() {
        let (bin, args) = command_plan(&req("r", "container", "nginx", "restart")).unwrap();
        assert_eq!(bin, "podman");
        assert_eq!(args, ["restart", "nginx"]);
        let (bin, args) = command_plan(&req("r", "vm", "win11", "stop")).unwrap();
        assert_eq!(bin, "virsh");
        assert_eq!(args, ["shutdown", "win11"]);
        assert!(command_plan(&req("r", "vm", "x", "explode")).is_none());
    }
}
