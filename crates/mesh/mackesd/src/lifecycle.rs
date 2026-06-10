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
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

/// The per-target request directory.
#[must_use]
pub fn lifecycle_dir(workgroup_root: &Path, target_host: &str) -> PathBuf {
    workgroup_root
        .join("fleet")
        .join("lifecycle")
        .join(target_host)
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
    let dir = lifecycle_dir(workgroup_root, target_host);
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
    let dir = lifecycle_dir(workgroup_root, self_host);
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
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(req) = serde_json::from_str::<LifecycleRequest>(&raw) {
                let _ = std::fs::remove_file(&p);
                out.push(req);
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
    let dir = lifecycle_dir(workgroup_root, target_host);
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
    let path = lifecycle_dir(workgroup_root, target_host).join(format!("{id}.result.json"));
    let raw = std::fs::read_to_string(&path).ok()?;
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
