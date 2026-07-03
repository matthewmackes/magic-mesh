//! EDITOR-9 (Part A) — the **project-tree side panel**: a lightweight file tree
//! over a root folder that the editor surface embeds beside the text widget.
//!
//! The tree is *lazy* — a directory's children are read from disk only when it is
//! first expanded (§7: a real [`std::fs::read_dir`], sorted dirs-first, with the
//! honest handling of an unreadable directory — a note, never a panic). A click on
//! a file row returns its path so the caller routes it through the EDITOR-3 open
//! seam ([`EditorSurface::open_path`](crate::panel::EditorSurface::open_path)); a
//! click on a directory row toggles its expansion.
//!
//! The read + expand/collapse state lives in [`ProjectTree`] and is exercised
//! headlessly (no egui) so the folding logic is unit-tested directly; the render
//! ([`show`]) is a thin token-styled (§4) walk over that state.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use mde_egui::egui::{self, Color32, RichText, Ui};
use mde_egui::Style;

/// One child row of a directory: a real filesystem entry (its absolute path, its
/// display name, and whether it is itself a directory). Sorted dirs-first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// The child's absolute path (the parent joined with its file name).
    path: PathBuf,
    /// The file-name component, rendered in the row.
    name: String,
    /// `true` when the entry is a directory (it sorts first + is expandable).
    is_dir: bool,
}

/// The lazily-read contents of one directory: its sorted children, plus the honest
/// reason it could not be read (§7 — an unreadable dir renders a note, not a panic).
#[derive(Debug, Clone)]
struct DirRead {
    /// The dirs-first, name-sorted children (empty when `error` is set).
    entries: Vec<Entry>,
    /// `Some(reason)` when [`std::fs::read_dir`] failed (missing / permission /
    /// not-a-directory); `None` on a clean read.
    error: Option<String>,
}

/// The project-tree panel state over one root folder.
///
/// Holds the root, the set of expanded directories, and the lazily-read cache of
/// each read directory's children. All disk reads go through [`read_dir_sorted`],
/// so the folding logic here is pure state and unit-testable without egui.
pub struct ProjectTree {
    /// The folder the tree is rooted at (always expanded + read on construction).
    root: PathBuf,
    /// The directories the operator has expanded (the root is inserted on `new`).
    expanded: BTreeSet<PathBuf>,
    /// The lazily-read children of every directory that has been expanded/read.
    cache: HashMap<PathBuf, DirRead>,
}

impl ProjectTree {
    /// Root a fresh tree at `root` and read its top level immediately, so a just-
    /// opened folder already lists its children (§7 — reachable on open).
    #[must_use]
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        let root = root.into();
        let mut tree = Self {
            root: root.clone(),
            expanded: BTreeSet::new(),
            cache: HashMap::new(),
        };
        tree.expanded.insert(root.clone());
        tree.ensure_read(&root);
        tree
    }

    /// The folder the tree is rooted at.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `dir` is currently expanded (its children are shown).
    #[must_use]
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Expand `dir`, reading its children lazily on the first expand. A no-op when
    /// `dir` is already expanded (the cached read is kept).
    pub fn expand(&mut self, dir: &Path) {
        self.ensure_read(dir);
        self.expanded.insert(dir.to_path_buf());
    }

    /// Collapse `dir` (its cached children are kept for the next expand).
    pub fn collapse(&mut self, dir: &Path) {
        self.expanded.remove(dir);
    }

    /// Flip `dir`'s expansion — the disclosure-triangle click. Reads lazily when it
    /// expands.
    pub fn toggle(&mut self, dir: &Path) {
        if self.is_expanded(dir) {
            self.collapse(dir);
        } else {
            self.expand(dir);
        }
    }

    /// Read `dir` into the cache if it has not been read yet (idempotent).
    fn ensure_read(&mut self, dir: &Path) {
        if !self.cache.contains_key(dir) {
            let read = read_dir_sorted(dir);
            self.cache.insert(dir.to_path_buf(), read);
        }
    }

    /// The cached children of `dir` (empty until it has been read/expanded).
    fn entries(&self, dir: &Path) -> &[Entry] {
        match self.cache.get(dir) {
            Some(read) => &read.entries,
            None => &[],
        }
    }

    /// The honest read error for `dir`, if [`std::fs::read_dir`] failed.
    fn read_error(&self, dir: &Path) -> Option<&str> {
        self.cache.get(dir).and_then(|read| read.error.as_deref())
    }
}

/// Read `dir` with a real [`std::fs::read_dir`], sorted dirs-first then
/// case-insensitive name (the same familiar order `mde-files` groups by). An
/// unreadable directory (missing / permission / not-a-directory) records the
/// error and an empty listing — honest (§7), never a panic.
fn read_dir_sorted(dir: &Path) -> DirRead {
    match std::fs::read_dir(dir) {
        Ok(reader) => {
            let mut entries: Vec<Entry> = reader
                .filter_map(std::result::Result::ok)
                .map(|de| {
                    // A real type probe; a failed `file_type` falls back to "file"
                    // (honest — an unreadable child is not treated as a directory).
                    let is_dir = de.file_type().is_ok_and(|t| t.is_dir());
                    Entry {
                        path: de.path(),
                        name: de.file_name().to_string_lossy().into_owned(),
                        is_dir,
                    }
                })
                .collect();
            entries.sort_by(entry_order);
            DirRead {
                entries,
                error: None,
            }
        }
        Err(err) => DirRead {
            entries: Vec::new(),
            error: Some(err.to_string()),
        },
    }
}

/// Order two entries dirs-first, then by case-insensitive name — a stable listing
/// order familiar from every file manager.
fn entry_order(a: &Entry, b: &Entry) -> Ordering {
    // `is_dir` true (a directory) sorts ahead of false (a file): compare `b` vs `a`
    // so `true` wins the head.
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
}

/// Render the project tree into `ui`, returning the path of the file the operator
/// clicked this frame (the caller routes it through the EDITOR-3 open seam). A
/// directory row toggles its own expansion; a file row yields its path. The read
/// stays lazy — a folder's children are read only when first expanded.
pub(crate) fn show(ui: &mut Ui, tree: &mut ProjectTree) -> Option<PathBuf> {
    let root = tree.root().to_path_buf();
    let root_name = root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(root_name)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .strong(),
        );
    });
    ui.separator();

    let mut opened: Option<PathBuf> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            render_dir(ui, tree, &root, Style::SP_S, &mut opened);
        });
    opened
}

/// Render the children of `dir` at horizontal `indent`, recursing into each
/// expanded child directory. A clicked file's path is written into `opened`.
fn render_dir(
    ui: &mut Ui,
    tree: &mut ProjectTree,
    dir: &Path,
    indent: f32,
    opened: &mut Option<PathBuf>,
) {
    if let Some(err) = tree.read_error(dir) {
        // The honest unreadable-directory face (§7) — a muted note, no panic.
        tree_note(ui, indent, &format!("\u{26A0} {err}"));
        return;
    }
    // Snapshot the children (owned) so the recursive `&mut tree` mutation (a
    // disclosure toggle) doesn't alias the borrow of the cache.
    let entries = tree.entries(dir).to_vec();
    for entry in entries {
        if entry.is_dir {
            let marker = if tree.is_expanded(&entry.path) {
                '\u{25BE}' // ▾ open
            } else {
                '\u{25B8}' // ▸ closed
            };
            if tree_row(ui, indent, &format!("{marker} {}", entry.name), Style::TEXT) {
                tree.toggle(&entry.path);
            }
            if tree.is_expanded(&entry.path) {
                render_dir(ui, tree, &entry.path, indent + Style::SP_M, opened);
            }
        } else if tree_row(ui, indent + Style::SP_M, &entry.name, Style::TEXT) {
            *opened = Some(entry.path.clone());
        }
    }
}

/// One clickable tree row at `indent`. Returns `true` when it was clicked.
fn tree_row(ui: &mut Ui, indent: f32, text: &str, color: Color32) -> bool {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.add(
            egui::Label::new(RichText::new(text).size(Style::SMALL).color(color))
                .sense(egui::Sense::click()),
        )
        .clicked()
    })
    .inner
}

/// A muted, non-interactive note at `indent` (the unreadable-directory state).
fn tree_note(ui: &mut Ui, indent: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.label(RichText::new(text).size(Style::SMALL).color(Style::WARN));
    });
}

#[cfg(test)]
mod tests {
    use super::{read_dir_sorted, ProjectTree};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique temp dir for a live tree test, cleaned up on drop (mirrors the
    /// `fileops` test idiom — the crate has no `tempfile` dev-dep).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-editor-tree-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&base).expect("create temp dir");
            Self(base)
        }
        fn join(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn read_dir_sorted_puts_dirs_first_then_case_insensitive_name() {
        let d = TempDir::new("sort");
        std::fs::create_dir(d.join("zeta")).expect("mkdir zeta");
        std::fs::create_dir(d.join("Alpha")).expect("mkdir Alpha");
        std::fs::write(d.join("b.txt"), b"b").expect("write b");
        std::fs::write(d.join("A.txt"), b"a").expect("write A");

        let read = read_dir_sorted(&d.0);
        assert!(read.error.is_none(), "a real dir reads cleanly");
        let names: Vec<&str> = read.entries.iter().map(|e| e.name.as_str()).collect();
        // Dirs first (Alpha, zeta — case-insensitive), then files (A.txt, b.txt).
        assert_eq!(names, vec!["Alpha", "zeta", "A.txt", "b.txt"]);
        assert!(read.entries[0].is_dir && read.entries[1].is_dir);
        assert!(!read.entries[2].is_dir && !read.entries[3].is_dir);
    }

    #[test]
    fn new_tree_lists_the_root_immediately() {
        let d = TempDir::new("root");
        std::fs::write(d.join("one.rs"), b"fn main() {}").expect("write");
        let tree = ProjectTree::new(d.0.clone());
        assert!(tree.is_expanded(&d.0), "the root opens expanded");
        let names: Vec<&str> = tree.entries(&d.0).iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["one.rs"]);
    }

    #[test]
    fn expand_and_collapse_fold_a_subdir_lazily() {
        let d = TempDir::new("fold");
        let sub = d.join("sub");
        std::fs::create_dir(&sub).expect("mkdir sub");
        std::fs::write(sub.join("nested.txt"), b"x").expect("write nested");

        let mut tree = ProjectTree::new(d.0.clone());
        // The subdir shows in the root but is NOT read/expanded until toggled.
        assert!(!tree.is_expanded(&sub), "a fresh subdir is collapsed");
        assert!(tree.entries(&sub).is_empty(), "unexpanded → not yet read");

        tree.toggle(&sub); // expand → lazy read
        assert!(tree.is_expanded(&sub));
        let nested: Vec<&str> = tree.entries(&sub).iter().map(|e| e.name.as_str()).collect();
        assert_eq!(nested, vec!["nested.txt"], "expand reads the children");

        tree.toggle(&sub); // collapse
        assert!(!tree.is_expanded(&sub), "toggle again collapses");
    }

    #[test]
    fn an_unreadable_directory_is_honest_not_a_panic() {
        let d = TempDir::new("bad");
        let missing = d.join("does-not-exist");
        let read = read_dir_sorted(&missing);
        assert!(
            read.entries.is_empty(),
            "no children from an unreadable dir"
        );
        assert!(read.error.is_some(), "the read error is surfaced honestly");

        // A tree rooted at a missing path still constructs (no panic) + reports it.
        let tree = ProjectTree::new(missing.clone());
        assert!(tree.read_error(&missing).is_some());
        assert!(tree.entries(&missing).is_empty());
    }
}
