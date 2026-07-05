//! KDC-MESH-7 — the node-targeted file browse (design #11b / #7).
//!
//! Each node exposes a set of **shared roots** — operator-designated directories
//! (e.g. `~/Public`, the mesh share) that a paired phone / the Phones hub may
//! browse over the overlay. This module is the honest, security-gated listing
//! primitive both the live browse verb and the service-directory snapshot use.
//!
//! **The security gate is the whole point.** "Pairing is enough" (design #16)
//! authorizes a paired phone to browse, but only *within* the shared roots — a
//! browse request is canonicalized and must resolve **inside** one of the roots,
//! so a paired phone can never walk out to `/etc/shadow` via `..` or a symlink.
//! A path outside every root is [`BrowseError::OutsideSharedRoots`], never a
//! listing (§7 — the gate is enforced in code + covered by tests).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One entry in a browsed directory — the row the phone / hub renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The entry's file name (not a full path — the browser joins it).
    pub name: String,
    /// Whether the entry is a directory (drill-in) vs a file.
    pub is_dir: bool,
    /// File size in bytes; `0` for directories.
    pub size: u64,
}

/// An operator-designated browseable root: a friendly `label` + its `path`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedRoot {
    /// The label shown in the browser (e.g. `Public`, `Mesh share`).
    pub label: String,
    /// The absolute directory path this root exposes.
    pub path: String,
}

impl SharedRoot {
    /// Build a root from a label + path.
    #[must_use]
    pub fn new(label: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
        }
    }
}

/// Why a browse was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseError {
    /// The requested path resolved outside every shared root — refused (the
    /// security gate: a paired phone browses inside the shared roots only).
    OutsideSharedRoots,
    /// The path exists but isn't a directory (nothing to list).
    NotADirectory,
    /// A filesystem error reading the directory (rendered reason).
    Io(String),
}

impl std::fmt::Display for BrowseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideSharedRoots => {
                f.write_str("path is outside the node's shared roots (refused)")
            }
            Self::NotADirectory => f.write_str("path is not a directory"),
            Self::Io(e) => write!(f, "read dir: {e}"),
        }
    }
}

impl std::error::Error for BrowseError {}

/// Shallow-list one directory into sorted [`FileEntry`] rows (dirs first, then
/// files, each alphabetical). Pure over the filesystem — no recursion, no
/// following into subdirectories. Used both for the live browse and for the
/// service-directory snapshot of a shared root's top level.
///
/// # Errors
/// [`BrowseError::NotADirectory`] if `dir` isn't a directory;
/// [`BrowseError::Io`] on a read error.
pub fn list_dir(dir: &Path) -> Result<Vec<FileEntry>, BrowseError> {
    if !dir.is_dir() {
        return Err(BrowseError::NotADirectory);
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| BrowseError::Io(e.to_string()))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        // A metadata failure (a dangling symlink, a race) shouldn't abort the whole
        // listing — skip the one entry honestly rather than fake a size/kind.
        let Ok(meta) = entry.metadata() else { continue };
        let is_dir = meta.is_dir();
        entries.push(FileEntry {
            name,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// Resolve `req` to a real path **inside** one of `roots`, or refuse.
///
/// An empty / `"/"` request is the roots' own listing (the caller handles that);
/// this resolves a concrete path. Canonicalizes both the request and each root
/// (so `..` and symlinks can't escape) and requires the request to live under a
/// root. Returns the canonical path when contained.
///
/// # Errors
/// [`BrowseError::OutsideSharedRoots`] when the path escapes every root or can't
/// be canonicalized within one.
pub fn resolve_within_roots(roots: &[SharedRoot], req: &str) -> Result<PathBuf, BrowseError> {
    let requested = PathBuf::from(req);
    // Canonicalize the request; a non-existent path or one that fails to resolve
    // is treated as outside (fail-closed, §7).
    let canon_req = requested
        .canonicalize()
        .map_err(|_| BrowseError::OutsideSharedRoots)?;
    for root in roots {
        let Ok(canon_root) = PathBuf::from(&root.path).canonicalize() else {
            continue;
        };
        if canon_req.starts_with(&canon_root) {
            return Ok(canon_req);
        }
    }
    Err(BrowseError::OutsideSharedRoots)
}

/// Browse a node's shared files (the node-targeted file browse, design #7/#11b).
///
/// With an empty / `"/"` request, lists the shared roots themselves as pseudo-
/// directory entries (each root is a top-level folder). Otherwise resolves the
/// request inside a root ([`resolve_within_roots`]) and lists it. A path outside
/// every root is refused — the security gate.
///
/// # Errors
/// [`BrowseError::OutsideSharedRoots`] for an out-of-bounds path; the listing
/// errors otherwise.
pub fn browse(roots: &[SharedRoot], req: &str) -> Result<Vec<FileEntry>, BrowseError> {
    let trimmed = req.trim();
    if trimmed.is_empty() || trimmed == "/" {
        // The roots' own level: each shared root is a browseable folder.
        return Ok(roots
            .iter()
            .map(|r| FileEntry {
                name: r.label.clone(),
                is_dir: true,
                size: 0,
            })
            .collect());
    }
    let path = resolve_within_roots(roots, trimmed)?;
    list_dir(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // shared/pub/{a.txt, sub/{b.txt}}
        let pubdir = tmp.path().join("pub");
        fs::create_dir_all(pubdir.join("sub")).unwrap();
        fs::write(pubdir.join("a.txt"), b"hello").unwrap();
        fs::write(pubdir.join("sub").join("b.txt"), b"hi").unwrap();
        // A sibling dir OUTSIDE the shared root — must never be browseable.
        fs::create_dir_all(tmp.path().join("secret")).unwrap();
        fs::write(tmp.path().join("secret").join("keys"), b"nope").unwrap();
        tmp
    }

    fn roots(tmp: &Path) -> Vec<SharedRoot> {
        vec![SharedRoot::new(
            "Public",
            tmp.join("pub").to_string_lossy().to_string(),
        )]
    }

    #[test]
    fn empty_request_lists_the_shared_roots() {
        let tmp = fixture();
        let entries = browse(&roots(tmp.path()), "").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Public");
        assert!(entries[0].is_dir);
    }

    #[test]
    fn browse_a_root_lists_its_contents_dirs_first() {
        let tmp = fixture();
        let pubdir = tmp.path().join("pub");
        let entries = browse(&roots(tmp.path()), &pubdir.to_string_lossy()).unwrap();
        // sub/ (dir) sorts before a.txt (file).
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "sub");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "a.txt");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, 5);
    }

    #[test]
    fn browse_a_subdir_within_a_root_is_allowed() {
        let tmp = fixture();
        let sub = tmp.path().join("pub").join("sub");
        let entries = browse(&roots(tmp.path()), &sub.to_string_lossy()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "b.txt");
    }

    #[test]
    fn a_path_outside_the_shared_roots_is_refused() {
        let tmp = fixture();
        let secret = tmp.path().join("secret");
        let r = browse(&roots(tmp.path()), &secret.to_string_lossy());
        assert_eq!(r, Err(BrowseError::OutsideSharedRoots));
    }

    #[test]
    fn a_dotdot_escape_out_of_a_root_is_refused() {
        // The security gate: `<root>/../secret` canonicalizes out of the root and
        // must be refused, not served.
        let tmp = fixture();
        let escape = tmp
            .path()
            .join("pub")
            .join("..")
            .join("secret")
            .to_string_lossy()
            .to_string();
        let r = browse(&roots(tmp.path()), &escape);
        assert_eq!(r, Err(BrowseError::OutsideSharedRoots));
    }

    #[test]
    fn resolve_within_roots_returns_the_canonical_contained_path() {
        let tmp = fixture();
        let sub = tmp.path().join("pub").join("sub");
        let got = resolve_within_roots(&roots(tmp.path()), &sub.to_string_lossy()).unwrap();
        assert_eq!(got, sub.canonicalize().unwrap());
    }

    #[test]
    fn list_dir_on_a_file_is_not_a_directory() {
        let tmp = fixture();
        let file = tmp.path().join("pub").join("a.txt");
        assert_eq!(list_dir(&file), Err(BrowseError::NotADirectory));
    }
}
