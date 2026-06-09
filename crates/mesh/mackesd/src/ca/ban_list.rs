//! EPIC-SEC-BANLIST (Q53) — compromised-node ban list.
//!
//! A ban list is a permanent record of node-ids that must never
//! re-join the mesh, even with a valid enrollment passcode and even
//! across a CA rotation. The motivating threat: a stolen lighthouse
//! or peer whose node-id an attacker could replay. CA revocation
//! retires a *cert*; the ban list retires an *identity*.
//!
//! ## Storage + mesh-wide union
//!
//! Each peer owns a ban list at
//! `<workgroup_root>/<self-node-id>/mackesd/ban-list.json` — the same
//! per-peer `<workgroup_root>/<node-id>/mackesd/` layout that pending
//! enrollments + bundles use (see
//! [`crate::nebula_enroll::pending_enroll_path`]). Because the
//! QNM-Shared / mesh-home root is GFS-replicated, a ban written on
//! one peer propagates to every peer.
//!
//! The enrollment gate checks the **union** of every peer's ban
//! list ([`load_union`]) — so a ban set on ANY peer blocks the
//! node-id everywhere, and a single surviving copy of the ban
//! outlasts the loss of the peer that set it.
//!
//! ## Fail-open per file, fail-safe on the gate
//!
//! [`load_union`] skips a peer subdir whose ban-list.json is missing
//! or malformed (logs + continues) rather than aborting the whole
//! union — one corrupt file mustn't blind the gate to every other
//! peer's bans. A missing root directory yields an empty union (the
//! pre-mesh-home boot path). The enrollment gate treats "node-id in
//! union" as a hard reject.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk ban list. One per peer, under
/// `<workgroup_root>/<node-id>/mackesd/ban-list.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanList {
    /// Banned node-ids (e.g. `peer:anvil`). A set, so a re-ban is
    /// idempotent + the on-disk order is deterministic.
    #[serde(default)]
    pub node_ids: BTreeSet<String>,
}

/// Error surface for ban-list I/O. Per-file load failures inside
/// [`load_union`] don't produce these — only direct
/// [`load_file`]/[`save_file`]/[`add_banned`] calls do.
#[derive(Debug)]
pub enum BanListError {
    /// Filesystem read/write/mkdir failure.
    Io(String),
    /// JSON (de)serialization failure on a directly-targeted file.
    Json(String),
}

impl std::fmt::Display for BanListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "ban-list I/O: {e}"),
            Self::Json(e) => write!(f, "ban-list JSON: {e}"),
        }
    }
}

impl std::error::Error for BanListError {}

const BAN_LIST_FILENAME: &str = "ban-list.json";

/// Per-peer ban-list path:
/// `<workgroup_root>/<self_node_id>/mackesd/ban-list.json`. Mirrors
/// [`crate::nebula_enroll::pending_enroll_path`]'s layout.
#[must_use]
pub fn ban_list_path(workgroup_root: &Path, self_node_id: &str) -> PathBuf {
    workgroup_root
        .join(self_node_id)
        .join("mackesd")
        .join(BAN_LIST_FILENAME)
}

/// Load a single ban-list file. Returns an empty [`BanList`] when
/// the file is missing (the common case — most peers never ban
/// anyone). A malformed file is the caller's problem here (returns
/// `Err`); [`load_union`] swallows that case instead.
///
/// # Errors
/// [`BanListError::Io`] on a read failure other than not-found;
/// [`BanListError::Json`] on malformed content.
pub fn load_file(path: &Path) -> Result<BanList, BanListError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| BanListError::Json(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BanList::default()),
        Err(e) => Err(BanListError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Atomic-write a ban list to `path` (temp + rename), creating the
/// parent `<node-id>/mackesd/` dir tree if needed.
///
/// # Errors
/// [`BanListError::Json`] on serialize failure; [`BanListError::Io`]
/// on mkdir/write/rename failure.
pub fn save_file(path: &Path, list: &BanList) -> Result<(), BanListError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| BanListError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(list).map_err(|e| BanListError::Json(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())
        .map_err(|e| BanListError::Io(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        BanListError::Io(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

/// Union of every peer's ban list under `workgroup_root`. Walks each
/// immediate `<workgroup_root>/<peer>/mackesd/ban-list.json`, unioning the
/// node-ids. Per-file failures (missing / malformed) are logged +
/// skipped so one bad file doesn't blind the gate. A missing or
/// unreadable `workgroup_root` yields an empty union.
#[must_use]
pub fn load_union(workgroup_root: &Path) -> BTreeSet<String> {
    let mut union = BTreeSet::new();
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(_) => return union, // pre-mesh-home boot: nothing banned.
    };
    for entry in entries.flatten() {
        let peer_dir = entry.path();
        if !peer_dir.is_dir() {
            continue;
        }
        let path = peer_dir.join("mackesd").join(BAN_LIST_FILENAME);
        if !path.exists() {
            continue;
        }
        match load_file(&path) {
            Ok(list) => union.extend(list.node_ids),
            Err(e) => tracing::warn!(
                target: "mackesd::ban_list",
                path = %path.display(),
                error = %e,
                "skipping malformed ban-list during union",
            ),
        }
    }
    union
}

/// `true` when `node_id` appears in any peer's ban list. Thin
/// convenience over [`load_union`] for the enrollment gate.
#[must_use]
pub fn is_banned(workgroup_root: &Path, node_id: &str) -> bool {
    load_union(workgroup_root).contains(node_id)
}

/// Add `node_id` to the local peer's ban list (creating the file +
/// dir tree on first ban). Idempotent — re-banning an already-listed
/// node-id is a no-op write-back. Returns `true` when the node-id
/// was newly added, `false` when it was already present.
///
/// # Errors
/// Per [`load_file`] / [`save_file`].
pub fn add_banned(
    workgroup_root: &Path,
    self_node_id: &str,
    node_id: &str,
) -> Result<bool, BanListError> {
    let path = ban_list_path(workgroup_root, self_node_id);
    let mut list = load_file(&path)?;
    let newly = list.node_ids.insert(node_id.to_string());
    save_file(&path, &list)?;
    Ok(newly)
}

/// Remove `node_id` from the LOCAL peer's ban list. Returns `true`
/// when an entry was removed, `false` when it wasn't present. Only
/// affects this peer's list — a ban another peer set still surfaces
/// in [`load_union`] until lifted there.
///
/// # Errors
/// Per [`load_file`] / [`save_file`].
pub fn remove_banned(
    workgroup_root: &Path,
    self_node_id: &str,
    node_id: &str,
) -> Result<bool, BanListError> {
    let path = ban_list_path(workgroup_root, self_node_id);
    let mut list = load_file(&path)?;
    let removed = list.node_ids.remove(node_id);
    if removed {
        save_file(&path, &list)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_peer_ban(workgroup_root: &Path, peer: &str, ids: &[&str]) {
        let list = BanList {
            node_ids: ids.iter().map(|s| s.to_string()).collect(),
        };
        save_file(&ban_list_path(workgroup_root, peer), &list).unwrap();
    }

    #[test]
    fn ban_list_path_mirrors_peer_layout() {
        let root = Path::new("/mesh");
        assert_eq!(
            ban_list_path(root, "peer:anvil"),
            Path::new("/mesh/peer:anvil/mackesd/ban-list.json")
        );
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nope/mackesd/ban-list.json");
        assert!(load_file(&p).unwrap().node_ids.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = ban_list_path(tmp.path(), "peer:self");
        let mut list = BanList::default();
        list.node_ids.insert("peer:evil".to_string());
        list.node_ids.insert("peer:stolen".to_string());
        save_file(&p, &list).unwrap();
        let back = load_file(&p).unwrap();
        assert_eq!(back, list);
        // Atomic write leaves no .tmp behind.
        assert!(!p.with_extension("json.tmp").exists());
    }

    #[test]
    fn load_union_combines_every_peer() {
        let tmp = tempfile::tempdir().unwrap();
        write_peer_ban(tmp.path(), "peer:a", &["peer:evil"]);
        write_peer_ban(tmp.path(), "peer:b", &["peer:stolen", "peer:evil"]);
        write_peer_ban(tmp.path(), "peer:c", &[]);
        let union = load_union(tmp.path());
        assert_eq!(union.len(), 2);
        assert!(union.contains("peer:evil"));
        assert!(union.contains("peer:stolen"));
    }

    #[test]
    fn load_union_missing_root_is_empty() {
        let union = load_union(Path::new("/nonexistent/mesh/root"));
        assert!(union.is_empty());
    }

    #[test]
    fn load_union_skips_malformed_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_peer_ban(tmp.path(), "peer:good", &["peer:evil"]);
        // Drop a corrupt ban-list under another peer.
        let bad = ban_list_path(tmp.path(), "peer:bad");
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::fs::write(&bad, b"{ not json").unwrap();
        // The good peer's ban still surfaces; the bad file is skipped.
        let union = load_union(tmp.path());
        assert_eq!(union.len(), 1);
        assert!(union.contains("peer:evil"));
    }

    #[test]
    fn is_banned_reflects_union() {
        let tmp = tempfile::tempdir().unwrap();
        write_peer_ban(tmp.path(), "peer:a", &["peer:evil"]);
        assert!(is_banned(tmp.path(), "peer:evil"));
        assert!(!is_banned(tmp.path(), "peer:innocent"));
    }

    #[test]
    fn add_banned_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(add_banned(tmp.path(), "peer:self", "peer:evil").unwrap());
        // Second add of the same id → not newly added.
        assert!(!add_banned(tmp.path(), "peer:self", "peer:evil").unwrap());
        let list = load_file(&ban_list_path(tmp.path(), "peer:self")).unwrap();
        assert_eq!(list.node_ids.len(), 1);
        assert!(is_banned(tmp.path(), "peer:evil"));
    }

    #[test]
    fn add_banned_accumulates_distinct_ids() {
        let tmp = tempfile::tempdir().unwrap();
        add_banned(tmp.path(), "peer:self", "peer:one").unwrap();
        add_banned(tmp.path(), "peer:self", "peer:two").unwrap();
        let list = load_file(&ban_list_path(tmp.path(), "peer:self")).unwrap();
        assert_eq!(list.node_ids.len(), 2);
    }
}
