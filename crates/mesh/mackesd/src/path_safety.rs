//! Phase 2.5 — path safety + allowed-roots resolver.
//!
//! Every path that arrives over `dev.mackes.MDE.Files.SendTo`
//! goes through [`PathPolicy::validate`] before mded touches the
//! filesystem. The policy enforces:
//!
//!   * **Canonicalisation** — resolve `..`, `.`, double slashes,
//!     and follow symlinks. Done with `std::fs::canonicalize` so
//!     the result is always absolute + symlink-free.
//!   * **Traversal rejection** — a raw `..` in the input is
//!     rejected before we even hit the filesystem; even after
//!     canonicalisation, the resolved path must still live inside
//!     one of the allowed roots.
//!   * **RBAC-allowed-roots** — the caller's role grants access to
//!     a list of allowed roots (per-user `~/Downloads`, the mesh
//!     drop, etc.). The validated path must be a descendant of at
//!     least one root.
//!
//! Pure-fn module — no DBus / async / I/O concerns above the
//! `canonicalize` call. The Send-To orchestrator (Phase 2.6) is
//! the one consumer.

use std::path::{Component, Path, PathBuf};

/// One configured root that a request is allowed to land inside.
/// Roots are canonicalised at policy-construction time so the
/// per-request validation only canonicalises the request payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedRoot {
    /// Canonical absolute path of the root directory.
    pub path: PathBuf,
    /// Free-form label for audit logs ("user-downloads",
    /// "mesh-drop", "fleet-config").
    pub label: String,
}

impl AllowedRoot {
    /// Construct from a candidate path. The path must exist;
    /// canonicalisation fails otherwise.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::NotFound`] if the path can't be
    /// canonicalised (most commonly: doesn't exist or insufficient
    /// permission to traverse).
    pub fn new(path: impl AsRef<Path>, label: impl Into<String>) -> Result<Self, PathError> {
        let canon = std::fs::canonicalize(path.as_ref())
            .map_err(|_| PathError::NotFound(path.as_ref().to_path_buf()))?;
        Ok(Self {
            path: canon,
            label: label.into(),
        })
    }
}

/// One set of allowed roots for one caller.
#[derive(Debug, Clone, Default)]
pub struct PathPolicy {
    roots: Vec<AllowedRoot>,
}

impl PathPolicy {
    /// Empty policy — rejects every path.
    #[must_use]
    pub fn empty() -> Self {
        Self { roots: Vec::new() }
    }

    /// Construct with a fixed set of roots.
    #[must_use]
    pub fn with_roots(roots: Vec<AllowedRoot>) -> Self {
        Self { roots }
    }

    /// Add a root to the policy.
    pub fn allow(&mut self, root: AllowedRoot) {
        self.roots.push(root);
    }

    /// Read-only view of every configured root.
    #[must_use]
    pub fn roots(&self) -> &[AllowedRoot] {
        &self.roots
    }

    /// Validate a candidate path. Returns the canonicalised path
    /// (always absolute + symlink-free) on success.
    ///
    /// # Errors
    ///
    /// * [`PathError::Traversal`] — input contains `..` literally.
    /// * [`PathError::NotFound`] — input doesn't exist / not
    ///   readable.
    /// * [`PathError::OutsideRoots`] — canonical path doesn't sit
    ///   under any allowed root.
    pub fn validate(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, PathError> {
        let candidate = candidate.as_ref();
        // Pre-canonicalise traversal reject: any literal `..` in
        // the request gets bounced before we hit the filesystem.
        // This avoids racing with symlink swaps where a `..` could
        // escape the root between canonicalise + validate.
        for c in candidate.components() {
            if matches!(c, Component::ParentDir) {
                return Err(PathError::Traversal(candidate.to_path_buf()));
            }
        }
        let canon = std::fs::canonicalize(candidate)
            .map_err(|_| PathError::NotFound(candidate.to_path_buf()))?;
        if !self.is_under_any_root(&canon) {
            return Err(PathError::OutsideRoots(canon));
        }
        Ok(canon)
    }

    /// Same as [`Self::validate`] but doesn't hit the filesystem.
    /// Used in tests + for "would this path be accepted?" UI
    /// previews. The caller is responsible for canonicalising
    /// the input first.
    #[must_use]
    pub fn would_accept_canonical(&self, canonical: &Path) -> bool {
        self.is_under_any_root(canonical)
    }

    fn is_under_any_root(&self, canonical: &Path) -> bool {
        self.roots.iter().any(|r| canonical.starts_with(&r.path))
    }
}

/// Errors raised by the path-safety layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Request contained a literal `..` traversal segment.
    Traversal(PathBuf),
    /// Path doesn't exist (or insufficient permission to
    /// canonicalise it).
    NotFound(PathBuf),
    /// Path is valid but outside every configured root.
    OutsideRoots(PathBuf),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Traversal(p) => {
                write!(f, "path: traversal segment rejected: {}", p.display())
            }
            Self::NotFound(p) => write!(f, "path: not found: {}", p.display()),
            Self::OutsideRoots(p) => {
                write!(f, "path: {} is outside the allowed-roots set", p.display())
            }
        }
    }
}

impl std::error::Error for PathError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    fn make_policy_for(roots: &[(&Path, &str)]) -> PathPolicy {
        let mut p = PathPolicy::empty();
        for (path, label) in roots {
            p.allow(AllowedRoot::new(path, *label).expect("canonicalise root"));
        }
        p
    }

    #[test]
    fn empty_policy_rejects_everything() {
        let tmp = tempdir().expect("tmpdir");
        let file = tmp.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        let p = PathPolicy::empty();
        let err = p.validate(&file).unwrap_err();
        assert!(matches!(err, PathError::OutsideRoots(_)));
    }

    #[test]
    fn validate_accepts_file_under_allowed_root() {
        let tmp = tempdir().expect("tmpdir");
        let file = tmp.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        let p = make_policy_for(&[(tmp.path(), "scratch")]);
        let canon = p.validate(&file).expect("accepted");
        assert!(canon.is_absolute());
    }

    #[test]
    fn validate_rejects_literal_parent_dir_in_request() {
        let tmp = tempdir().expect("tmpdir");
        let file = tmp.path().join("a.txt");
        fs::write(&file, b"x").unwrap();
        let p = make_policy_for(&[(tmp.path(), "scratch")]);
        let traversal = tmp.path().join("..").join("escape");
        let err = p.validate(&traversal).unwrap_err();
        assert!(matches!(err, PathError::Traversal(_)));
    }

    #[test]
    fn validate_rejects_nonexistent_path() {
        let tmp = tempdir().expect("tmpdir");
        let p = make_policy_for(&[(tmp.path(), "scratch")]);
        let err = p.validate(tmp.path().join("ghost.txt")).unwrap_err();
        assert!(matches!(err, PathError::NotFound(_)));
    }

    #[test]
    fn validate_rejects_path_outside_roots() {
        let root = tempdir().expect("root tmpdir");
        let outside = tempdir().expect("outside tmpdir");
        let other_file = outside.path().join("a.txt");
        fs::write(&other_file, b"x").unwrap();
        let p = make_policy_for(&[(root.path(), "scratch")]);
        let err = p.validate(&other_file).unwrap_err();
        assert!(matches!(err, PathError::OutsideRoots(_)));
    }

    #[test]
    fn validate_resolves_symlinks_through_root() {
        let root = tempdir().expect("root tmpdir");
        let real = root.path().join("real.txt");
        fs::write(&real, b"x").unwrap();
        let link = root.path().join("link.txt");
        symlink(&real, &link).expect("symlink");
        let p = make_policy_for(&[(root.path(), "scratch")]);
        let canon = p.validate(&link).expect("symlink under root accepted");
        // canonicalize follows symlinks → ends at real.txt.
        assert_eq!(canon.file_name().and_then(|s| s.to_str()), Some("real.txt"));
    }

    #[test]
    fn validate_rejects_symlink_escaping_root() {
        let root = tempdir().expect("root tmpdir");
        let outside = tempdir().expect("outside tmpdir");
        let real = outside.path().join("real.txt");
        fs::write(&real, b"x").unwrap();
        let link = root.path().join("escape.txt");
        symlink(&real, &link).expect("symlink");
        let p = make_policy_for(&[(root.path(), "scratch")]);
        let err = p.validate(&link).unwrap_err();
        assert!(
            matches!(err, PathError::OutsideRoots(_)),
            "symlink target outside root must be rejected"
        );
    }

    #[test]
    fn would_accept_canonical_does_not_hit_filesystem() {
        let tmp = tempdir().expect("tmpdir");
        let p = make_policy_for(&[(tmp.path(), "scratch")]);
        let fake = tmp.path().join("does-not-exist.txt");
        // would_accept_canonical accepts the path because it sits
        // under the root, regardless of existence.
        assert!(p.would_accept_canonical(&fake));
    }

    #[test]
    fn allowed_root_construction_canonicalises() {
        let tmp = tempdir().expect("tmpdir");
        let nested = tmp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let r = AllowedRoot::new(&nested, "x").expect("canonicalise");
        assert!(r.path.is_absolute());
    }

    #[test]
    fn allowed_root_rejects_nonexistent() {
        let err = AllowedRoot::new("/does/not/exist/9999", "x").unwrap_err();
        assert!(matches!(err, PathError::NotFound(_)));
    }

    #[test]
    fn policy_with_multiple_roots_accepts_each() {
        let a = tempdir().expect("tmpdir a");
        let b = tempdir().expect("tmpdir b");
        let fa = a.path().join("aa.txt");
        let fb = b.path().join("bb.txt");
        fs::write(&fa, b"x").unwrap();
        fs::write(&fb, b"x").unwrap();
        let p = make_policy_for(&[(a.path(), "a"), (b.path(), "b")]);
        assert!(p.validate(&fa).is_ok());
        assert!(p.validate(&fb).is_ok());
    }

    #[test]
    fn path_error_display_includes_path() {
        let e = PathError::NotFound(PathBuf::from("/x"));
        assert!(format!("{e}").contains("/x"));
        let e = PathError::OutsideRoots(PathBuf::from("/y"));
        assert!(format!("{e}").contains("/y"));
        let e = PathError::Traversal(PathBuf::from("/z"));
        assert!(format!("{e}").contains("/z"));
    }
}
