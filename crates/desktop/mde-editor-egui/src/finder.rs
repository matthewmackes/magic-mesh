//! EDITOR-7 (part A) — the **fuzzy file-finder overlay**: an overlay text field +
//! ranked result list over the files under the editor's project root.
//!
//! `Cmd`/`Ctrl-P` opens it (the intercept lives at the panel level in
//! [`crate::panel`], never in the text widget). Opening walks the root — a bounded
//! recursive [`std::fs::read_dir`] crawl that skips `target/` and `.git/` (§7: a
//! bounded, honest walk, not an unbounded crawl; an unreadable directory is simply
//! skipped, never a panic). Typing fuzzy-filters the list through [`crate::fuzzy`];
//! the arrow keys move the selection; Enter opens the highlighted file through
//! [`EditorSurface::open_path`](crate::EditorSurface::open_path); Esc dismisses.
//!
//! The state + hit-logic ([`FileFinder`] with [`walk_project`], the ranked
//! [`results`](FileFinder::results), selection movement, and [`pick`](FileFinder::pick))
//! are pure and unit-tested without egui; [`show`] is a thin token-styled (§4)
//! render over that state.

// `module_name_repetitions`: `FileFinder` is the domain name for this module's one
// public type; trimming the echo of the `finder` module reads worse. `missing_const_for_fn`
// (nursery) is over-eager on the small mutators — the same allow `buffer.rs` makes.
#![allow(clippy::module_name_repetitions, clippy::missing_const_for_fn)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use mde_egui::egui::{self, Align2, Key, Modifiers, RichText, ScrollArea, Vec2};
use mde_egui::Style;

use crate::fuzzy;

/// Hard cap on the files the project walk collects, so a huge tree can't blow
/// memory or stall the finder — a bounded walk (§7), not an unbounded crawl.
const WALK_FILE_CAP: usize = 20_000;
/// Hard cap on recursion depth (belt-and-braces against a very deep or symlink-
/// looped tree; `file_type` already treats a symlink as a non-directory, so a
/// symlinked cycle is walked as a leaf, never recursed).
const WALK_DEPTH_CAP: usize = 32;
/// Directory names the walk skips wholesale — build output + VCS metadata.
const SKIP_DIRS: [&str; 2] = ["target", ".git"];

/// Fixed width of the finder overlay plate (§4 spacing units).
const FINDER_WIDTH: f32 = Style::SP_XL * 16.0;
/// Max height of the scrolling result list before it scrolls (§4 spacing units).
const LIST_MAX_H: f32 = Style::SP_XL * 9.0;
/// Vertical drop of the overlay from the top edge (§4 spacing units).
const TOP_DROP: f32 = Style::SP_XL * 2.0;

/// Walk `root` recursively and collect every regular file's path, skipping the
/// `target/` and `.git/` directories and bounding both total files and depth.
///
/// A bounded, honest crawl (§7): an unreadable directory is skipped, never a
/// panic. Each directory's entries are name-sorted before recursion, so the
/// resulting listing is deterministic.
fn walk_project(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_into(root, 0, &mut files);
    files
}

/// Recurse one directory into `files`, honouring the file + depth caps.
fn walk_into(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > WALK_DEPTH_CAP || files.len() >= WALK_FILE_CAP {
        return;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return; // an unreadable directory is skipped, not a panic (§7)
    };
    let mut entries: Vec<std::fs::DirEntry> = reader.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if files.len() >= WALK_FILE_CAP {
            return;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !is_skipped(&entry.file_name()) {
                walk_into(&entry.path(), depth + 1, files);
            }
        } else {
            files.push(entry.path());
        }
    }
}

/// Whether directory `name` is one the walk skips (`target` / `.git`).
fn is_skipped(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// A candidate's root-relative display string (falls back to the full path if it
/// somehow isn't under `root`), the text both shown and fuzzy-matched.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The fuzzy file-finder overlay state (EDITOR-7).
///
/// Holds the walked candidate list, their root-relative display strings, the live
/// query, and the highlighted row. `open`/`close`/`results`/`pick` are pure state
/// so the folding + ranking logic is unit-tested without egui; [`show`] renders it.
#[derive(Default)]
pub struct FileFinder {
    /// Whether the overlay is currently shown.
    open: bool,
    /// Every file under the walked root — the pick targets.
    candidates: Vec<PathBuf>,
    /// Each candidate's root-relative display string, parallel to `candidates`.
    display: Vec<String>,
    /// The live query text.
    query: String,
    /// The highlighted row — an index into the *ranked results*, not `candidates`.
    selected: usize,
    /// Set on open so the query field grabs the keyboard for one frame.
    focus_query: bool,
}

impl FileFinder {
    /// Whether the finder overlay is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the finder rooted at `root`: walk it for candidates, reset the query +
    /// selection, and request keyboard focus. The one entry point the `Cmd`/`Ctrl-P`
    /// intercept calls (see [`crate::panel`]).
    pub fn open_at(&mut self, root: &Path) {
        self.candidates = walk_project(root);
        self.display = self
            .candidates
            .iter()
            .map(|p| display_path(root, p))
            .collect();
        self.query.clear();
        self.selected = 0;
        self.open = true;
        self.focus_query = true;
    }

    /// Close the overlay (Esc, or after a pick).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The candidate indices matching the query, best-first (the ranked results).
    fn results(&self) -> Vec<usize> {
        fuzzy::ranked(
            &self.query,
            self.display.iter().map(String::as_str).enumerate(),
        )
    }

    /// Move the highlighted row one step within `len` results, saturating at the
    /// ends (a finder list doesn't wrap).
    fn move_selection(&mut self, forward: bool, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let last = len - 1;
        let cur = self.selected.min(last);
        self.selected = if forward {
            (cur + 1).min(last)
        } else {
            cur.saturating_sub(1)
        };
    }

    /// The path of the highlighted result, if any — what Enter / a click opens.
    fn pick(&self, results: &[usize]) -> Option<PathBuf> {
        results
            .get(self.selected)
            .and_then(|&idx| self.candidates.get(idx))
            .cloned()
    }
}

/// Render the finder overlay on `ctx` and return the file the operator opened this
/// frame (Enter on the highlighted row, or a click), closing the overlay on a pick
/// or Esc. A no-op returning `None` while the overlay is closed.
///
/// The nav chords are consumed here — before the query field reads them — so the
/// arrows move the selection (not the text caret), Enter opens, and Esc dismisses.
pub fn show(ctx: &egui::Context, finder: &mut FileFinder) -> Option<PathBuf> {
    if !finder.open {
        return None;
    }

    let results = finder.results();
    if finder.selected >= results.len() {
        finder.selected = results.len().saturating_sub(1);
    }

    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        finder.close();
        return None;
    }
    if up {
        finder.move_selection(false, results.len());
    }
    if down {
        finder.move_selection(true, results.len());
    }

    let mut picked = if enter { finder.pick(&results) } else { None };

    egui::Window::new("Go to file")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_DROP))
        .show(ctx, |ui| {
            ui.set_min_width(FINDER_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Go to file")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);

            let field = ui.add(
                egui::TextEdit::singleline(&mut finder.query)
                    .hint_text("Type to fuzzy-find a file\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut finder.focus_query) {
                field.request_focus();
            }
            ui.add_space(Style::SP_XS);
            ui.separator();

            if results.is_empty() {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(if finder.candidates.is_empty() {
                        "No files under this folder"
                    } else {
                        "No matching files"
                    })
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .italics(),
                );
                return;
            }

            ScrollArea::vertical()
                .id_salt("editor-finder-list")
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (row, &idx) in results.iter().enumerate() {
                        if result_row(ui, &finder.display[idx], row == finder.selected) {
                            finder.selected = row;
                            picked = finder.candidates.get(idx).cloned();
                        }
                    }
                });
        });

    if picked.is_some() {
        finder.close();
    }
    picked
}

/// One selectable result row: the root-relative path, highlighted when selected.
/// Returns `true` when clicked. Token-styled (§4).
fn result_row(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    ui.selectable_label(
        selected,
        RichText::new(label).size(Style::SMALL).color(Style::TEXT),
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::{walk_project, FileFinder};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique temp dir for a live finder test, cleaned up on drop (the crate has
    /// no `tempfile` dep for the GUI modules — the same idiom `project_tree`'s
    /// tests use).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-editor-finder-{tag}-{}-{}",
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
    fn walk_finds_a_seeded_file_and_skips_target_and_git() {
        let d = TempDir::new("walk");
        // A real source file nested one level down (the hit we want).
        std::fs::create_dir(d.join("src")).expect("mkdir src");
        std::fs::write(d.join("src/lib.rs"), b"fn main() {}").expect("write lib.rs");
        // Noise the walk must skip: build output + VCS metadata.
        std::fs::create_dir(d.join("target")).expect("mkdir target");
        std::fs::write(d.join("target/artifact.rlib"), b"x").expect("write artifact");
        std::fs::create_dir(d.join(".git")).expect("mkdir .git");
        std::fs::write(d.join(".git/HEAD"), b"ref: x").expect("write HEAD");

        let files = walk_project(&d.0);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&d.0)
                    .unwrap_or(p.as_path())
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(
            names.contains(&"src/lib.rs".to_owned()),
            "the seeded file is found"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("target/")),
            "the target/ directory is skipped"
        );
        assert!(
            !names.iter().any(|n| n.starts_with(".git/")),
            "the .git/ directory is skipped"
        );
        assert_eq!(
            files.len(),
            1,
            "only the real source file survives the walk"
        );
    }

    #[test]
    fn open_at_walks_and_the_query_ranks_the_matching_file() {
        // The pure finder path a select→open drives: open over a real dir, filter,
        // and the highlighted pick is the file whose name matches the query.
        let d = TempDir::new("pick");
        std::fs::write(d.join("alpha.rs"), b"a").expect("write alpha");
        std::fs::write(d.join("target_file.rs"), b"t").expect("write target");
        std::fs::write(d.join("zeta.rs"), b"z").expect("write zeta");

        let mut finder = FileFinder::default();
        finder.open_at(&d.0);
        assert!(finder.is_open(), "open_at opens the overlay");
        assert_eq!(finder.candidates.len(), 3, "all three files are candidates");

        // Empty query lists everything; a query narrows + ranks the hit to the top.
        assert_eq!(finder.results().len(), 3, "empty query lists every file");
        finder.query = "target".to_owned();
        let results = finder.results();
        finder.selected = 0;
        let picked = finder.pick(&results).expect("a match is highlighted");
        assert_eq!(
            picked.file_name().and_then(|n| n.to_str()),
            Some("target_file.rs"),
            "the query ranks the matching file first, and pick yields its path"
        );
    }

    #[test]
    fn move_selection_saturates_at_the_ends() {
        let mut finder = FileFinder::default();
        // Up at the top stays at 0; down past the end clamps to the last row.
        finder.move_selection(false, 3);
        assert_eq!(finder.selected, 0, "up at the top saturates");
        finder.move_selection(true, 3);
        finder.move_selection(true, 3);
        finder.move_selection(true, 3);
        finder.move_selection(true, 3);
        assert_eq!(
            finder.selected, 2,
            "down past the end clamps to the last row"
        );
    }
}
