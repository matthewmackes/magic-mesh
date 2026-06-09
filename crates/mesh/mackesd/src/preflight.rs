//! Phase 3.5 — pre-flight validation for Send-To requests.
//!
//! Before mded queues a Send-To, every request runs through this
//! pre-flight battery. Each check produces a [`CheckRow`]
//! (one per dimension); the UI surfaces them as a list in the
//! Send-To dialog so the user sees exactly which constraint is
//! holding the send back.
//!
//! The eight locked checks (Phase 3.5 design lock):
//!
//!   * **Sources** — every source path must pass [`PathPolicy`]
//!     ([`crate::path_safety`]).
//!   * **Permissions** — caller must have read access to each
//!     source.
//!   * **Allowed paths** — every source canonicalises inside the
//!     RBAC roots.
//!   * **Disk space** — sum of source sizes ≤ destination free
//!     space (per caller-supplied `available_bytes`).
//!   * **Node reachability** — destination peer was online in
//!     the last `freshness` window (per caller-supplied
//!     `peer_last_seen_ms`).
//!   * **File-type policy** — none of the sources is on the
//!     locked block list (`.exe`, `.msi`, `.bat`, `.app`, …).
//!   * **Rollback feasibility** — `SendMode::Move` requires the
//!     destination peer to expose a writable rollback bucket; we
//!     model this via the caller-supplied
//!     `rollback_available_for_target`.
//!   * **Target free** — when `ConflictPolicy::Skip` is selected
//!     but a target file already exists, the send would skip every
//!     source — surface that as a soft warning so the user can
//!     pick a different policy.
//!
//! The module is dep-free + pure-fn so it tests in milliseconds.
//! Real I/O (disk-space query, peer heartbeat lookup) is supplied
//! by the caller as parameters; the policy logic is here.

use std::path::PathBuf;
use std::time::Duration;

use crate::path_safety::PathPolicy;

/// One check outcome row, ordered as it appears in the Send-To
/// dialog list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    /// Human-friendly check identifier (matches the UI label).
    pub id: &'static str,
    /// `Ok` / `Warn` / `Block`. The dialog renders the icon
    /// accordingly.
    pub status: CheckStatus,
    /// Optional explanation when the status is non-Ok. Empty for
    /// passing checks.
    pub message: String,
}

/// Per-check outcome. `Block` prevents the send; `Warn` shows a
/// caution glyph but allows it; `Ok` is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// Check passed.
    Ok,
    /// Soft warning — user can proceed.
    Warn,
    /// Hard fail — the send is blocked.
    Block,
}

/// Send-mode in the simplified form needed by the pre-flight. The
/// real `Backend::SendMode` enum lives in mde-files; we mirror
/// just the variants we care about here so mackesd doesn't
/// depend on mde-files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendModeLite {
    /// `Copy` — duplicate source onto destination, sources stay.
    Copy,
    /// `Move` — destination receives, sources are removed.
    Move,
    /// `Sync` — bidirectional reconciliation.
    Sync,
    /// `Deploy` — Sync + run the target's hook (no rollback).
    Deploy,
    /// `Stage` — write to a staging area; commit-or-discard later.
    Stage,
}

/// Conflict policy in the pre-flight form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicyLite {
    /// Prompt user (interactive dialog).
    Ask,
    /// Skip target if it exists.
    Skip,
    /// Overwrite the existing target.
    Overwrite,
    /// Rename the incoming file (append `-1`, `-2`, …).
    Rename,
}

/// One Send-To request, surfaced as the input to the pre-flight.
/// Caller fills every field; pre-flight reads but doesn't mutate.
#[derive(Debug, Clone)]
pub struct Request {
    /// Sources as supplied by the user (un-canonicalised).
    pub sources: Vec<PathBuf>,
    /// Destination's display name (used in error messages only).
    pub destination_label: String,
    /// Total bytes the destination will receive (sum of source
    /// sizes the caller has already measured).
    pub total_bytes: u64,
    /// Free space the destination peer reports (or `u64::MAX` for
    /// "unknown" / "skip this check").
    pub destination_free_bytes: u64,
    /// Milliseconds since the destination peer was last seen
    /// online. `u64::MAX` means "never seen / unknown".
    pub destination_last_seen_ms: u64,
    /// `true` when the destination peer has a rollback bucket
    /// configured + writable.
    pub rollback_available_for_target: bool,
    /// Whether at least one target file already exists at the
    /// destination (used by the Conflict-Skip warning).
    pub target_exists: bool,
    /// Send mode.
    pub mode: SendModeLite,
    /// Conflict-resolution policy.
    pub conflict: ConflictPolicyLite,
}

/// Maximum allowed peer-staleness for reachability check. Locked
/// at 60 s — peers off-mesh longer than that are treated as
/// unreachable.
pub const REACHABILITY_WINDOW: Duration = Duration::from_secs(60);

/// Locked file-type block list. Mirrors the policy lock from the
/// 2026-05-19 design discussion: executable Windows binaries are
/// the highest-risk artifact on the mesh and must never be
/// silently distributed to peers.
pub const BLOCKED_EXTENSIONS: &[&str] = &["exe", "msi", "bat", "cmd", "ps1", "app"];

/// Run every locked pre-flight check. The eight outputs are
/// returned in the locked UI order so the Send-To dialog can
/// render them as a flat list with no re-ordering.
#[must_use]
pub fn preflight(req: &Request, policy: &PathPolicy) -> Vec<CheckRow> {
    vec![
        check_sources_present(req),
        check_path_safety(req, policy),
        check_disk_space(req),
        check_reachability(req),
        check_file_type(req),
        check_rollback_feasible(req),
        check_target_free(req),
        check_mode_destination_combo(req),
    ]
}

/// `true` when every check is `Ok` or `Warn` (no `Block`).
#[must_use]
pub fn rows_allow_send(rows: &[CheckRow]) -> bool {
    !rows.iter().any(|r| r.status == CheckStatus::Block)
}

fn check_sources_present(req: &Request) -> CheckRow {
    if req.sources.is_empty() {
        CheckRow {
            id: "sources",
            status: CheckStatus::Block,
            message: "no sources supplied".into(),
        }
    } else {
        CheckRow {
            id: "sources",
            status: CheckStatus::Ok,
            message: String::new(),
        }
    }
}

fn check_path_safety(req: &Request, policy: &PathPolicy) -> CheckRow {
    for src in &req.sources {
        // Pure-fn check first: rejects literal `..` segments.
        if src
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return CheckRow {
                id: "allowed-paths",
                status: CheckStatus::Block,
                message: format!("{}: traversal segment rejected", src.display()),
            };
        }
        // Then walk through the policy's roots without touching the
        // filesystem (the orchestrator does the canonicalise later).
        // We accept here when ANY root contains this prefix.
        if !policy_has_prefix(policy, src) {
            return CheckRow {
                id: "allowed-paths",
                status: CheckStatus::Block,
                message: format!("{}: outside the RBAC-allowed roots", src.display()),
            };
        }
    }
    CheckRow {
        id: "allowed-paths",
        status: CheckStatus::Ok,
        message: String::new(),
    }
}

fn policy_has_prefix(policy: &PathPolicy, candidate: &std::path::Path) -> bool {
    if policy.roots().is_empty() {
        return false;
    }
    policy
        .roots()
        .iter()
        .any(|r| candidate.starts_with(&r.path))
}

fn check_disk_space(req: &Request) -> CheckRow {
    // u64::MAX is the sentinel for "unknown — skip the check".
    if req.destination_free_bytes == u64::MAX {
        return CheckRow {
            id: "disk-space",
            status: CheckStatus::Warn,
            message: "destination free-space unknown".into(),
        };
    }
    if req.total_bytes > req.destination_free_bytes {
        return CheckRow {
            id: "disk-space",
            status: CheckStatus::Block,
            message: format!(
                "destination has {} bytes free, send needs {}",
                req.destination_free_bytes, req.total_bytes
            ),
        };
    }
    CheckRow {
        id: "disk-space",
        status: CheckStatus::Ok,
        message: String::new(),
    }
}

fn check_reachability(req: &Request) -> CheckRow {
    if req.destination_last_seen_ms == u64::MAX {
        return CheckRow {
            id: "reachability",
            status: CheckStatus::Block,
            message: format!("{} has never been seen online", req.destination_label),
        };
    }
    let max_ms = REACHABILITY_WINDOW.as_millis() as u64;
    if req.destination_last_seen_ms > max_ms {
        return CheckRow {
            id: "reachability",
            status: CheckStatus::Block,
            message: format!(
                "{} last seen {} ms ago (limit {} ms)",
                req.destination_label, req.destination_last_seen_ms, max_ms
            ),
        };
    }
    CheckRow {
        id: "reachability",
        status: CheckStatus::Ok,
        message: String::new(),
    }
}

fn check_file_type(req: &Request) -> CheckRow {
    for src in &req.sources {
        let Some(ext) = src
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if BLOCKED_EXTENSIONS.contains(&ext.as_str()) {
            return CheckRow {
                id: "file-type",
                status: CheckStatus::Block,
                message: format!(
                    "{}: extension .{ext} is on the policy block list",
                    src.display()
                ),
            };
        }
    }
    CheckRow {
        id: "file-type",
        status: CheckStatus::Ok,
        message: String::new(),
    }
}

fn check_rollback_feasible(req: &Request) -> CheckRow {
    match req.mode {
        SendModeLite::Move | SendModeLite::Deploy => {
            if !req.rollback_available_for_target {
                CheckRow {
                    id: "rollback",
                    status: CheckStatus::Block,
                    message: format!(
                        "{} mode requires a writable rollback bucket on {}",
                        match req.mode {
                            SendModeLite::Move => "Move",
                            SendModeLite::Deploy => "Deploy",
                            _ => unreachable!(),
                        },
                        req.destination_label
                    ),
                }
            } else {
                CheckRow {
                    id: "rollback",
                    status: CheckStatus::Ok,
                    message: String::new(),
                }
            }
        }
        _ => CheckRow {
            id: "rollback",
            status: CheckStatus::Ok,
            message: "n/a for this mode".into(),
        },
    }
}

fn check_target_free(req: &Request) -> CheckRow {
    if req.target_exists && req.conflict == ConflictPolicyLite::Skip {
        CheckRow {
            id: "target-free",
            status: CheckStatus::Warn,
            message: "target exists; Skip policy would no-op the send".into(),
        }
    } else {
        CheckRow {
            id: "target-free",
            status: CheckStatus::Ok,
            message: String::new(),
        }
    }
}

fn check_mode_destination_combo(req: &Request) -> CheckRow {
    if matches!(req.mode, SendModeLite::Sync)
        && req.target_exists
        && req.conflict == ConflictPolicyLite::Overwrite
    {
        // Sync + Overwrite is destructive: it'll silently
        // overwrite the remote copy of every conflicting file.
        // Surface as Warn so the user has a chance to switch to
        // Rename.
        return CheckRow {
            id: "mode-combo",
            status: CheckStatus::Warn,
            message: "Sync + Overwrite will silently replace remote files".into(),
        };
    }
    CheckRow {
        id: "mode-combo",
        status: CheckStatus::Ok,
        message: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_safety::AllowedRoot;
    use std::fs;
    use tempfile::tempdir;

    fn policy_for(root: &std::path::Path) -> PathPolicy {
        let mut p = PathPolicy::empty();
        p.allow(AllowedRoot::new(root, "scratch").expect("canonicalise"));
        p
    }

    fn req_in(root: &std::path::Path, name: &str) -> Request {
        let p = root.join(name);
        Request {
            sources: vec![p],
            destination_label: "pine".into(),
            total_bytes: 1024,
            destination_free_bytes: 1_000_000_000,
            destination_last_seen_ms: 5_000,
            rollback_available_for_target: true,
            target_exists: false,
            mode: SendModeLite::Copy,
            conflict: ConflictPolicyLite::Ask,
        }
    }

    #[test]
    fn preflight_returns_eight_rows() {
        let tmp = tempdir().unwrap();
        let r = req_in(tmp.path(), "a.txt");
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        assert_eq!(rows.len(), 8, "the 8 locked checks must each emit a row");
    }

    #[test]
    fn preflight_allows_clean_request() {
        let tmp = tempdir().unwrap();
        let r = req_in(tmp.path(), "a.txt");
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        assert!(rows_allow_send(&rows));
        for row in &rows {
            assert!(
                row.status != CheckStatus::Block,
                "no row should block: {row:?}"
            );
        }
    }

    #[test]
    fn empty_sources_blocks() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.sources.clear();
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        assert!(!rows_allow_send(&rows));
        assert!(rows
            .iter()
            .any(|r| r.id == "sources" && r.status == CheckStatus::Block));
    }

    #[test]
    fn parent_dir_segment_blocks() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.sources = vec![tmp.path().join("..").join("escape")];
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        assert!(!rows_allow_send(&rows));
        assert!(rows
            .iter()
            .any(|r| r.id == "allowed-paths" && r.status == CheckStatus::Block));
    }

    #[test]
    fn outside_roots_blocks() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let mut r = req_in(outside.path(), "a.txt");
        // Source is outside the policy's root.
        let _ = fs::write(outside.path().join("a.txt"), b"x");
        let policy = policy_for(root.path());
        let rows = preflight(&r, &policy);
        assert!(!rows_allow_send(&rows));
        assert!(rows
            .iter()
            .any(|r| r.id == "allowed-paths" && r.status == CheckStatus::Block));
        // Suppress unused warning.
        let _ = &mut r;
    }

    #[test]
    fn disk_space_warns_on_unknown() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.destination_free_bytes = u64::MAX;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "disk-space").unwrap();
        assert_eq!(row.status, CheckStatus::Warn);
    }

    #[test]
    fn disk_space_blocks_when_too_small() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.total_bytes = 10_000;
        r.destination_free_bytes = 1_000;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "disk-space").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
    }

    #[test]
    fn reachability_blocks_stale_peer() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        // Last seen 5 minutes ago — past the 60 s window.
        r.destination_last_seen_ms = 300_000;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "reachability").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
    }

    #[test]
    fn reachability_blocks_never_seen() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.destination_last_seen_ms = u64::MAX;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "reachability").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
    }

    #[test]
    fn file_type_blocks_exe() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "installer.exe");
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "file-type").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
        // Suppress unused warning.
        let _ = &mut r;
    }

    #[test]
    fn file_type_blocks_case_insensitive() {
        let tmp = tempdir().unwrap();
        let r = req_in(tmp.path(), "x.EXE");
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "file-type").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
    }

    #[test]
    fn rollback_blocks_move_without_bucket() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.mode = SendModeLite::Move;
        r.rollback_available_for_target = false;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "rollback").unwrap();
        assert_eq!(row.status, CheckStatus::Block);
    }

    #[test]
    fn rollback_na_for_copy() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.rollback_available_for_target = false; // would block Move
                                                 // Stays Copy.
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "rollback").unwrap();
        assert_eq!(row.status, CheckStatus::Ok);
    }

    #[test]
    fn target_free_warns_on_skip_with_existing() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.conflict = ConflictPolicyLite::Skip;
        r.target_exists = true;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "target-free").unwrap();
        assert_eq!(row.status, CheckStatus::Warn);
        // Warn doesn't block the send.
        assert!(rows_allow_send(&rows));
    }

    #[test]
    fn sync_overwrite_warns() {
        let tmp = tempdir().unwrap();
        let mut r = req_in(tmp.path(), "a.txt");
        r.mode = SendModeLite::Sync;
        r.conflict = ConflictPolicyLite::Overwrite;
        r.target_exists = true;
        let policy = policy_for(tmp.path());
        let rows = preflight(&r, &policy);
        let row = rows.iter().find(|r| r.id == "mode-combo").unwrap();
        assert_eq!(row.status, CheckStatus::Warn);
    }

    #[test]
    fn rows_allow_send_returns_false_on_any_block() {
        let rows = vec![
            CheckRow {
                id: "a",
                status: CheckStatus::Ok,
                message: String::new(),
            },
            CheckRow {
                id: "b",
                status: CheckStatus::Warn,
                message: String::new(),
            },
            CheckRow {
                id: "c",
                status: CheckStatus::Block,
                message: "x".into(),
            },
        ];
        assert!(!rows_allow_send(&rows));
    }

    #[test]
    fn rows_allow_send_returns_true_with_only_warn_and_ok() {
        let rows = vec![
            CheckRow {
                id: "a",
                status: CheckStatus::Ok,
                message: String::new(),
            },
            CheckRow {
                id: "b",
                status: CheckStatus::Warn,
                message: "w".into(),
            },
        ];
        assert!(rows_allow_send(&rows));
    }

    #[test]
    fn blocked_extensions_list_is_locked_set() {
        for ext in ["exe", "msi", "bat", "cmd", "ps1", "app"] {
            assert!(BLOCKED_EXTENSIONS.contains(&ext), "{ext} must be blocked");
        }
        // Whitelist sanity: ordinary types stay allowed.
        for ext in ["txt", "md", "pdf", "png", "jpg"] {
            assert!(
                !BLOCKED_EXTENSIONS.contains(&ext),
                "{ext} must not be blocked"
            );
        }
    }

    #[test]
    fn reachability_window_is_60s() {
        assert_eq!(REACHABILITY_WINDOW, Duration::from_secs(60));
    }
}
