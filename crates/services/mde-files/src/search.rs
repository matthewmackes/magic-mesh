//! Phase 1.8 — search-results view (pure-fn filter).
//!
//! When the toolbar's search input is non-empty, the main pane
//! switches from the per-view list (mesh overview / peer folder /
//! local pins) to a flat results list, filtered across the current
//! scope.
//!
//! This module ships the pure data-side: a case-insensitive,
//! whitespace-trimming substring filter over [`FileRow`]. The view
//! layer plugs it into the visible list — that integration lives
//! with the Iced view-functions, not here.
//!
//! Match policy (locked 2026-05-19):
//!
//!   * Trim leading + trailing whitespace from the query before
//!     matching. An all-whitespace query matches nothing (treated
//!     as empty).
//!   * Match against `FileRow::name` and `FileRow::origin()` as a
//!     pair — "type the filename OR the peer name" both work.
//!   * Case-insensitive — `ASCII` only; Unicode case folding lands
//!     when we move to user-data backends (Phase 2.3+).
//!   * Empty / whitespace query returns the full input unchanged so
//!     the caller can use one helper for "search on, search off".

use std::path::{Path, PathBuf};

use crate::model::FileRow;

/// Apply the locked search policy to one row.
///
/// Empty / whitespace-only queries match everything (used so the
/// caller doesn't have to branch on "is search active?").
#[must_use]
pub fn matches_query(row: &FileRow, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    let q_lower = q.to_ascii_lowercase();
    let name = row.name.to_ascii_lowercase();
    if name.contains(&q_lower) {
        return true;
    }
    if let Some(origin) = row.origin() {
        if origin.to_ascii_lowercase().contains(&q_lower) {
            return true;
        }
    }
    false
}

/// Filter a slice of rows in place. Returns owned `FileRow`s so
/// the call site can take ownership for the view tree.
#[must_use]
pub fn filter_rows(rows: &[FileRow], query: &str) -> Vec<FileRow> {
    rows.iter()
        .filter(|r| matches_query(r, query))
        .cloned()
        .collect()
}

/// `true` when the query carries actual matchable characters
/// after the locked trim. The view code uses this to decide
/// whether to swap the main pane for the search-results view —
/// "search on" if-and-only-if `is_active(&search)`.
#[must_use]
pub fn is_active(query: &str) -> bool {
    !query.trim().is_empty()
}

// ───────────────────────── recursive filesystem search ─────────────────────
//
// The UI filter above narrows an *already-listed* directory. E11.6 parity also
// needs the file manager's "search this folder (and below)" — a recursive walk
// of the real filesystem. Same locked match policy (trimmed substring; ASCII
// case-insensitive by default), applied to each entry's file name.

/// Knobs for [`search_tree`]. `Default` = case-insensitive, 1000-result cap,
/// unlimited depth.
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    /// Match case-sensitively (default `false` — the locked UI policy).
    pub case_sensitive: bool,
    /// Stop after this many hits (`0` = unlimited). Caps an unbounded walk.
    pub max_results: usize,
    /// Maximum directory levels to descend (`0` = unlimited).
    pub max_depth: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            max_results: 1000,
            max_depth: 0,
        }
    }
}

/// One filesystem search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsHit {
    /// Absolute (or root-relative) path of the matching entry.
    pub path: PathBuf,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

/// Recursively search `root` for entries whose **file name** contains `query`
/// (trimmed; ASCII case-insensitive unless [`SearchOptions::case_sensitive`]).
///
/// An empty / whitespace-only query returns no hits (a recursive walk that
/// "matches everything" would just dump the tree). Symbolic links are **not**
/// descended, so the walk can't loop on a symlink cycle; an unreadable
/// subdirectory is skipped, not fatal. Iterative (explicit stack) to avoid deep
/// recursion on large trees.
#[must_use]
pub fn search_tree(root: &Path, query: &str, opts: &SearchOptions) -> Vec<FsHit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let needle = if opts.case_sensitive {
        q.to_string()
    } else {
        q.to_ascii_lowercase()
    };
    let mut hits = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if opts.max_results != 0 && hits.len() >= opts.max_results {
                return hits;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let haystack = if opts.case_sensitive {
                name.to_string_lossy().into_owned()
            } else {
                name.to_string_lossy().to_ascii_lowercase()
            };
            if haystack.contains(&needle) {
                hits.push(FsHit {
                    path: entry.path(),
                    is_dir: ft.is_dir(),
                });
            }
            // Descend into real subdirectories only (a symlink's file_type is
            // `is_symlink`, never `is_dir`, so symlink cycles are unreachable).
            if ft.is_dir() && (opts.max_depth == 0 || depth + 1 <= opts.max_depth) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mime;

    fn row_local(name: &'static str) -> FileRow {
        FileRow::local(name, Mime::Doc, "1 KB", "now")
    }

    fn row_mesh(name: &'static str, peer: &'static str) -> FileRow {
        FileRow::local(name, Mime::Doc, "1 KB", "now").with_mesh(peer)
    }

    #[test]
    fn empty_query_matches_everything() {
        let r = row_local("anything.txt");
        assert!(matches_query(&r, ""));
        assert!(matches_query(&r, "   "));
        assert!(matches_query(&r, "\t"));
    }

    #[test]
    fn substring_in_name_matches() {
        let r = row_local("important-notes.md");
        assert!(matches_query(&r, "notes"));
        assert!(matches_query(&r, "important"));
        assert!(matches_query(&r, ".md"));
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        let r = row_local("NOTES.MD");
        assert!(matches_query(&r, "notes"));
        assert!(matches_query(&r, "NoTeS"));
    }

    #[test]
    fn nonmatching_query_returns_false() {
        let r = row_local("alpha.txt");
        assert!(!matches_query(&r, "beta"));
        assert!(!matches_query(&r, "zzz"));
    }

    #[test]
    fn query_matches_origin_peer_name() {
        let r = row_mesh("data.bin", "pine.mesh");
        assert!(matches_query(&r, "pine"));
        assert!(matches_query(&r, "Pine"));
        assert!(matches_query(&r, "mesh"));
        // The filename alone still works.
        assert!(matches_query(&r, "data"));
    }

    #[test]
    fn whitespace_around_query_is_trimmed() {
        let r = row_local("notes.md");
        assert!(matches_query(&r, "  notes "));
        assert!(matches_query(&r, "\tnotes\n"));
    }

    #[test]
    fn filter_rows_returns_only_matches() {
        let rows = vec![
            row_local("alpha.txt"),
            row_local("beta.txt"),
            row_mesh("data.bin", "pine.mesh"),
        ];
        let out = filter_rows(&rows, "alpha");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "alpha.txt");

        let out = filter_rows(&rows, "pine");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "data.bin");

        // Empty query keeps everything.
        let out = filter_rows(&rows, "");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn is_active_only_for_non_empty_queries() {
        assert!(!is_active(""));
        assert!(!is_active("   "));
        assert!(!is_active("\t\n"));
        assert!(is_active("x"));
        assert!(is_active("  x  "));
    }

    #[test]
    fn filter_with_no_match_returns_empty() {
        let rows = vec![row_local("x"), row_local("y")];
        let out = filter_rows(&rows, "z");
        assert!(out.is_empty());
    }

    // ───────────────────── recursive filesystem search ─────────────────────

    use std::path::PathBuf;

    /// Build a small tree under a unique temp dir:
    ///   root/report.txt
    ///   root/notes.md
    ///   root/sub/report-2.txt
    ///   root/sub/deep/report-3.log
    fn tree(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("mde-files-search-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        std::fs::write(root.join("report.txt"), b"a").unwrap();
        std::fs::write(root.join("notes.md"), b"b").unwrap();
        std::fs::write(root.join("sub/report-2.txt"), b"c").unwrap();
        std::fs::write(root.join("sub/deep/report-3.log"), b"d").unwrap();
        root
    }

    fn names(hits: &[FsHit]) -> Vec<String> {
        let mut v: Vec<String> = hits
            .iter()
            .map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn search_tree_finds_matches_at_every_depth() {
        let root = tree("depthall");
        let hits = search_tree(&root, "report", &SearchOptions::default());
        assert_eq!(
            names(&hits),
            vec!["report-2.txt", "report-3.log", "report.txt"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_tree_empty_query_returns_nothing() {
        let root = tree("empty");
        assert!(search_tree(&root, "  ", &SearchOptions::default()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_tree_respects_case_sensitivity() {
        let root = tree("case");
        let insens = search_tree(&root, "REPORT", &SearchOptions::default());
        assert_eq!(insens.len(), 3, "case-insensitive by default");
        let sens = search_tree(
            &root,
            "REPORT",
            &SearchOptions {
                case_sensitive: true,
                ..SearchOptions::default()
            },
        );
        assert!(sens.is_empty(), "no upper-case names exist");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_tree_honours_max_depth_and_max_results() {
        let root = tree("limits");
        // depth 1: scan root + its immediate subdir, not sub/deep.
        let shallow = search_tree(
            &root,
            "report",
            &SearchOptions {
                max_depth: 1,
                ..SearchOptions::default()
            },
        );
        assert_eq!(names(&shallow), vec!["report-2.txt", "report.txt"]);
        // cap the result count.
        let capped = search_tree(
            &root,
            "report",
            &SearchOptions {
                max_results: 1,
                ..SearchOptions::default()
            },
        );
        assert_eq!(capped.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_tree_does_not_follow_symlink_cycles() {
        let root = tree("cycle");
        // a symlink pointing back at the root would loop a naive walk.
        std::os::unix::fs::symlink(&root, root.join("sub/loop")).unwrap();
        // must terminate, and still find the real files.
        let hits = search_tree(&root, "report", &SearchOptions::default());
        assert_eq!(hits.len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_tree_dir_names_match_too() {
        let root = tree("dirs");
        let hits = search_tree(&root, "deep", &SearchOptions::default());
        assert_eq!(hits.len(), 1);
        assert!(hits[0].is_dir, "the matching entry is a directory");
        let _ = std::fs::remove_dir_all(&root);
    }
}
