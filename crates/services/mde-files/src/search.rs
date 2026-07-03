//! FILEMGR-4 — async recursive search.
//!
//! A **streaming, cancellable** recursive search whose hits are ordinary
//! [`FileRow`]s — so a result set drops straight into a normal file view and
//! every op (copy / move / delete / Send-To) applies to a result exactly as it
//! would to a directory listing (§6 — glue over the existing model, not a new
//! surface). Two match kinds compose with AND semantics:
//!
//!   * **name-glob** — a shell glob (`*.rs`, `report-*.pdf`) matched against each
//!     entry's file name, compiled once with [`globset`].
//!   * **content grep** — a substring or regex matched against a file's raw bytes
//!     with the binary-safe [`regex::bytes`] engine (already in the workspace
//!     lockfile). Never a UTF-8 decode, so it greps binaries without panicking.
//!
//! plus **type / size / mtime** filters. The traversal reuses the [`FileOps`]
//! seam ([`crate::fileops`]) rather than `walkdir`/`ignore`, so the whole
//! fold — recursion, matching, filtering, cancellation — is unit-tested against
//! the in-memory [`FakeFileOps`](crate::fileops::FakeFileOps) with zero disk I/O,
//! and runs unchanged over a real filesystem *or* a mounted mesh path (an sshfs
//! mount is just another set of `read_dir`/`metadata`/`read_file` calls — slower,
//! honestly, but never a hang: the search streams whatever the mount yields and a
//! cancel stops it mid-flight).
//!
//! Layering (§6): this module is render-agnostic — no GUI toolkit. The desktop
//! surface (`mde-files-egui`) drives [`SearchRun`] on a worker thread and drains
//! its hits into a tab each frame; the pure fold [`run_search`] is what the tests
//! exercise directly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::SystemTime;

use crate::backend::{fmt_age, fmt_bytes, mime_of};
use crate::fileops::{FileOps, FileStat, LiveFileOps};
use crate::model::FileRow;

/// Files larger than this are not content-grepped (they still match on name /
/// type / size / mtime). A grep loads the whole file into memory, so this caps
/// the worst case; an over-cap file is honestly *not* a content hit rather than
/// stalling the search on a multi-gigabyte blob.
pub const DEFAULT_MAX_CONTENT_BYTES: u64 = 8 * 1024 * 1024;

/// Which entry kinds a search returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TypeFilter {
    /// Files, directories, and symlinks alike.
    #[default]
    Any,
    /// Regular files only.
    FilesOnly,
    /// Directories only.
    DirsOnly,
}

impl TypeFilter {
    /// The three filters, in toolbar order.
    pub const ALL: [Self; 3] = [Self::Any, Self::FilesOnly, Self::DirsOnly];

    /// The control label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Any => "Any",
            Self::FilesOnly => "Files",
            Self::DirsOnly => "Folders",
        }
    }

    /// Does an entry of this stat pass the type filter?
    #[must_use]
    fn accepts(self, stat: &FileStat) -> bool {
        match self {
            Self::Any => true,
            Self::FilesOnly => stat.is_file,
            Self::DirsOnly => stat.is_dir,
        }
    }
}

/// How the content query is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentMode {
    /// A literal substring (regex-escaped before compiling).
    #[default]
    Substring,
    /// A full regular expression.
    Regex,
}

/// A content grep sub-query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentQuery {
    /// The pattern text (a substring, or a regex when `mode` is [`ContentMode::Regex`]).
    pub pattern: String,
    /// Substring vs. regex.
    pub mode: ContentMode,
    /// Case-insensitive matching.
    pub case_insensitive: bool,
}

/// The type / extension / size / mtime filters, all optional (a `None` bound is
/// unbounded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// File / dir / any.
    pub kinds: TypeFilter,
    /// Restrict to this extension (no dot, matched case-insensitively). Implies
    /// files, since a directory has no meaningful extension here.
    pub ext: Option<String>,
    /// Minimum size in bytes (files only).
    pub min_size: Option<u64>,
    /// Maximum size in bytes (files only).
    pub max_size: Option<u64>,
    /// Only entries modified at/after this instant.
    pub modified_after: Option<SystemTime>,
    /// Only entries modified at/before this instant.
    pub modified_before: Option<SystemTime>,
}

impl Filters {
    /// `true` when any size bound is set (which makes the entry a file concept).
    fn has_size_bound(&self) -> bool {
        self.min_size.is_some() || self.max_size.is_some()
    }
}

/// The user-facing (uncompiled) search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// A name-glob matched against each entry's file name (`*.rs`, `report-*`).
    pub name_glob: Option<String>,
    /// Case-insensitive name matching.
    pub name_case_insensitive: bool,
    /// A content grep over file bytes.
    pub content: Option<ContentQuery>,
    /// Type / extension / size / mtime filters.
    pub filters: Filters,
    /// Skip content-grepping files larger than this (bytes).
    pub max_content_bytes: u64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            name_glob: None,
            name_case_insensitive: true,
            content: None,
            filters: Filters::default(),
            max_content_bytes: DEFAULT_MAX_CONTENT_BYTES,
        }
    }
}

impl SearchQuery {
    /// `true` when this query has at least one active predicate; an all-empty
    /// query would match every entry (a full recursive listing), which the UI
    /// guards against before spawning.
    #[must_use]
    pub fn is_meaningful(&self) -> bool {
        self.name_glob
            .as_ref()
            .is_some_and(|g| !g.trim().is_empty())
            || self
                .content
                .as_ref()
                .is_some_and(|c| !c.pattern.trim().is_empty())
            || self.filters.ext.is_some()
            || self.filters.has_size_bound()
            || self.filters.modified_after.is_some()
            || self.filters.modified_before.is_some()
            || self.filters.kinds != TypeFilter::Any
    }

    /// Compile the glob + content patterns once, up front. A malformed glob or
    /// regex is an honest typed [`SearchError`] the caller surfaces — never a
    /// silent empty result.
    pub fn compile(&self) -> Result<CompiledQuery, SearchError> {
        let name = match self.name_glob.as_ref().filter(|g| !g.trim().is_empty()) {
            Some(glob) => Some(
                globset::GlobBuilder::new(glob.trim())
                    .case_insensitive(self.name_case_insensitive)
                    .literal_separator(false)
                    .build()
                    .map_err(SearchError::Glob)?
                    .compile_matcher(),
            ),
            None => None,
        };
        let content = match self
            .content
            .as_ref()
            .filter(|c| !c.pattern.trim().is_empty())
        {
            Some(cq) => {
                let pat = match cq.mode {
                    ContentMode::Substring => regex::escape(&cq.pattern),
                    ContentMode::Regex => cq.pattern.clone(),
                };
                Some(
                    regex::bytes::RegexBuilder::new(&pat)
                        .case_insensitive(cq.case_insensitive)
                        // Big files are grepped as one buffer; keep the compiled
                        // program bounded so a pathological pattern can't blow up.
                        .size_limit(16 * 1024 * 1024)
                        .build()
                        .map_err(|e| SearchError::Regex(e.to_string()))?,
                )
            }
            None => None,
        };
        Ok(CompiledQuery {
            name,
            content,
            filters: self.filters.clone(),
            max_content_bytes: self.max_content_bytes,
        })
    }
}

/// A [`SearchQuery`] with its glob + regex compiled — the form the traversal
/// evaluates against every entry.
#[derive(Debug, Clone)]
pub struct CompiledQuery {
    name: Option<globset::GlobMatcher>,
    content: Option<regex::bytes::Regex>,
    filters: Filters,
    max_content_bytes: u64,
}

impl CompiledQuery {
    /// Does `path` (with pre-fetched `stat`) satisfy every active predicate? The
    /// content grep is the only predicate that reads the file, and only when all
    /// the cheap predicates already pass — so a big tree isn't read byte-for-byte
    /// unless the name / type / size / mtime filters have already selected it.
    fn matches<F: FileOps>(&self, ops: &F, path: &Path, stat: &FileStat) -> bool {
        if !self.filters.kinds.accepts(stat) {
            return false;
        }
        // An extension / size / content predicate is a file concept.
        if let Some(want) = &self.filters.ext {
            if !stat.is_file || !ext_eq(path, want) {
                return false;
            }
        }
        if self.filters.has_size_bound() {
            if !stat.is_file {
                return false;
            }
            if let Some(min) = self.filters.min_size {
                if stat.len < min {
                    return false;
                }
            }
            if let Some(max) = self.filters.max_size {
                if stat.len > max {
                    return false;
                }
            }
        }
        if self.filters.modified_after.is_some() || self.filters.modified_before.is_some() {
            let Some(m) = stat.modified else {
                return false;
            };
            if let Some(after) = self.filters.modified_after {
                if m < after {
                    return false;
                }
            }
            if let Some(before) = self.filters.modified_before {
                if m > before {
                    return false;
                }
            }
        }
        if let Some(matcher) = &self.name {
            let name = path.file_name().unwrap_or(path.as_os_str());
            if !matcher.is_match(Path::new(name)) {
                return false;
            }
        }
        if let Some(re) = &self.content {
            // Content is a regular-file concept; a directory/symlink never matches.
            if !stat.is_file || stat.len > self.max_content_bytes {
                return false;
            }
            let Ok(bytes) = ops.read_file(path) else {
                return false;
            };
            if !re.is_match(&bytes) {
                return false;
            }
        }
        true
    }
}

/// The tally a search returns when it finishes (or is cancelled).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Entries visited (stat'd), whether or not they matched.
    pub scanned: u64,
    /// Entries that matched every predicate (== the number of emitted hits).
    pub matched: u64,
    /// `true` when the cancel signal stopped the walk before it finished.
    pub cancelled: bool,
}

/// A streaming-search error: a bad glob / regex, or a failure to spawn the worker
/// thread. Always an honest typed failure the surface shows — never a silent
/// empty result.
#[derive(Debug)]
pub enum SearchError {
    /// The name-glob pattern didn't compile.
    Glob(globset::Error),
    /// The content regex didn't compile (message captured — `regex::Error` is
    /// not `Clone`, and this keeps the type simple to move across threads).
    Regex(String),
    /// The OS refused to spawn the search worker thread.
    Spawn(std::io::Error),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Glob(e) => write!(f, "bad name pattern: {e}"),
            Self::Regex(e) => write!(f, "bad content pattern: {e}"),
            Self::Spawn(e) => write!(f, "couldn't start search: {e}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// `true` when `path`'s extension equals `want` (case-insensitive, no dot).
fn ext_eq(path: &Path, want: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(want))
}

/// Shape a matched entry into a [`FileRow`] carrying its absolute path — the same
/// row shape a directory listing produces, so a hit is a fully-operable row
/// (its `path` feeds copy / move / delete / Send-To directly).
fn row_from(path: &Path, stat: &FileStat) -> FileRow {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mime = mime_of(path, stat.is_dir);
    let size = if stat.is_dir {
        "\u{2014}".to_string()
    } else {
        fmt_bytes(stat.len)
    };
    let age = stat
        .modified
        .map_or_else(|| "\u{2014}".to_string(), fmt_age);
    let display = if stat.is_dir {
        format!("{name}/")
    } else {
        name
    };
    FileRow::local(display, mime, size, age).with_path(path.to_string_lossy().into_owned())
}

/// Walk `root` recursively through `ops`, emitting each matching entry as a
/// [`FileRow`] to `on_hit` the moment it's found (streaming — never a blocking
/// collect), and stopping promptly whenever `cancel()` returns `true`.
///
/// Recursion is an explicit work-stack (no call-stack blow-up on a deep tree).
/// Symlinks are stat'd but never descended, so a symlink cycle can't spin the
/// walk. An unreadable directory (a permission wall on a mounted path) is
/// skipped, not fatal — the search keeps yielding what it *can* read.
///
/// `root` itself is the scope, not a candidate; its descendants are the hits.
pub fn run_search<F: FileOps>(
    ops: &F,
    query: &CompiledQuery,
    root: &Path,
    cancel: &dyn Fn() -> bool,
    on_hit: &mut dyn FnMut(FileRow),
) -> SearchStats {
    let mut stats = SearchStats::default();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel() {
            stats.cancelled = true;
            return stats;
        }
        let Ok(children) = ops.read_dir(&dir) else {
            // Unreadable (permission, race, or `dir` is a file) — skip, don't abort.
            continue;
        };
        for child in children {
            if cancel() {
                stats.cancelled = true;
                return stats;
            }
            let Ok(stat) = ops.symlink_metadata(&child) else {
                continue;
            };
            stats.scanned += 1;
            // Descend into real subdirectories only (never through a symlink).
            if stat.is_dir && !stat.is_symlink {
                stack.push(child.clone());
            }
            if query.matches(ops, &child, &stat) {
                stats.matched += 1;
                on_hit(row_from(&child, &stat));
            }
        }
    }
    stats
}

// ═══════════════════════════════════════════════════════════════════════════
// SearchRun — the off-thread streaming handle the desktop surface drives.
// ═══════════════════════════════════════════════════════════════════════════

/// One event from a live [`SearchRun`]: a streamed hit, or the terminal tally.
#[derive(Debug, Clone)]
pub enum SearchEvent {
    /// A matched entry (a fully-operable [`FileRow`] with an absolute path).
    Hit(FileRow),
    /// The walk finished (or was cancelled) — carries the final [`SearchStats`].
    Done(SearchStats),
}

/// A running recursive search over the **real** filesystem: spawned on its own
/// thread ([`LiveFileOps`]), it streams [`SearchEvent`]s over a channel the
/// surface drains each frame, and stops promptly when [`cancel`](Self::cancel)
/// is called. Dropping the handle leaves the detached worker to notice the
/// receiver is gone and exit.
pub struct SearchRun {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<SearchEvent>,
    finished: bool,
}

impl SearchRun {
    /// Compile `query`, then spawn the walk of `root` on a worker thread. A bad
    /// pattern fails *before* the thread starts; a thread-spawn refusal is an
    /// honest [`SearchError::Spawn`].
    pub fn spawn(query: &SearchQuery, root: PathBuf) -> Result<Self, SearchError> {
        let compiled = query.compile()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<SearchEvent>();
        let worker_cancel = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("mde-files-search".to_string())
            .spawn(move || {
                let ops = LiveFileOps::new();
                let is_cancelled = || worker_cancel.load(Ordering::Relaxed);
                let mut on_hit = |row: FileRow| {
                    // A send error means the surface dropped the receiver
                    // (search cleared / window closed); the next cancel check
                    // ends the walk — nothing to do here.
                    let _ = tx.send(SearchEvent::Hit(row));
                };
                let stats = run_search(&ops, &compiled, &root, &is_cancelled, &mut on_hit);
                let _ = tx.send(SearchEvent::Done(stats));
            })
            .map_err(SearchError::Spawn)?;
        Ok(Self {
            cancel,
            rx,
            finished: false,
        })
    }

    /// Signal the worker to stop at the next entry boundary. Idempotent.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `true` once a cancel has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `true` once a terminal [`SearchEvent::Done`] has been drained.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Drain every event currently available (non-blocking). The surface calls
    /// this once per frame and folds the hits into the results tab; a
    /// [`SearchEvent::Done`] flips [`finished`](Self::finished).
    pub fn drain(&mut self) -> Vec<SearchEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            if matches!(ev, SearchEvent::Done(_)) {
                self.finished = true;
            }
            out.push(ev);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileops::FakeFileOps;
    use std::cell::Cell;
    use std::time::Duration;

    // ── a small in-memory tree over FakeFileOps ────────────────────────────

    /// Build a fixed tree:
    /// ```text
    /// /r/a.txt            "alpha content"
    /// /r/b.rs             "fn beta() {}"
    /// /r/notes.md         "the needle is here"
    /// /r/sub/             (dir)
    /// /r/sub/c.rs         "fn gamma() {}"
    /// /r/sub/deep/        (dir)
    /// /r/sub/deep/d.txt   "delta needle"
    /// ```
    fn tree() -> FakeFileOps {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/r")).unwrap();
        fs.seed_file("/r/a.txt", b"alpha content").unwrap();
        fs.seed_file("/r/b.rs", b"fn beta() {}").unwrap();
        fs.seed_file("/r/notes.md", b"the needle is here").unwrap();
        fs.create_dir(Path::new("/r/sub")).unwrap();
        fs.seed_file("/r/sub/c.rs", b"fn gamma() {}").unwrap();
        fs.create_dir(Path::new("/r/sub/deep")).unwrap();
        fs.seed_file("/r/sub/deep/d.txt", b"delta needle").unwrap();
        fs
    }

    /// Collect the hit names for `query` over the tree, never cancelling.
    fn hits(fs: &FakeFileOps, query: &SearchQuery) -> (Vec<String>, SearchStats) {
        let compiled = query.compile().expect("compile");
        let mut names = Vec::new();
        let stats = run_search(fs, &compiled, Path::new("/r"), &|| false, &mut |row| {
            names.push(row.name.clone())
        });
        names.sort();
        (names, stats)
    }

    fn name_query(glob: &str) -> SearchQuery {
        SearchQuery {
            name_glob: Some(glob.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn name_glob_matches_recursively_across_subdirs() {
        let fs = tree();
        let (names, stats) = hits(&fs, &name_query("*.rs"));
        assert_eq!(names, vec!["b.rs", "c.rs"]);
        assert_eq!(stats.matched, 2);
        // scanned covers every entry in the whole tree (files + dirs).
        assert_eq!(stats.scanned, 7);
        assert!(!stats.cancelled);
    }

    #[test]
    fn name_glob_is_case_insensitive_by_default() {
        let fs = tree();
        let (names, _) = hits(&fs, &name_query("*.RS"));
        assert_eq!(names, vec!["b.rs", "c.rs"]);
    }

    #[test]
    fn name_glob_can_match_a_directory() {
        let fs = tree();
        let (names, _) = hits(&fs, &name_query("deep"));
        assert_eq!(names, vec!["deep/"]);
    }

    #[test]
    fn content_substring_greps_file_bytes_recursively() {
        let fs = tree();
        let q = SearchQuery {
            content: Some(ContentQuery {
                pattern: "needle".to_string(),
                mode: ContentMode::Substring,
                case_insensitive: true,
            }),
            ..Default::default()
        };
        let (names, stats) = hits(&fs, &q);
        assert_eq!(names, vec!["d.txt", "notes.md"]);
        assert_eq!(stats.matched, 2);
    }

    #[test]
    fn content_regex_matches_a_pattern() {
        let fs = tree();
        let q = SearchQuery {
            content: Some(ContentQuery {
                pattern: r"fn \w+\(\)".to_string(),
                mode: ContentMode::Regex,
                case_insensitive: false,
            }),
            ..Default::default()
        };
        let (names, _) = hits(&fs, &q);
        assert_eq!(names, vec!["b.rs", "c.rs"]);
    }

    #[test]
    fn name_and_content_compose_with_and_semantics() {
        let fs = tree();
        // *.md that also contains "needle" → notes.md; *.rs never contains it.
        let q = SearchQuery {
            name_glob: Some("*.md".to_string()),
            content: Some(ContentQuery {
                pattern: "needle".to_string(),
                mode: ContentMode::Substring,
                case_insensitive: true,
            }),
            ..Default::default()
        };
        let (names, _) = hits(&fs, &q);
        assert_eq!(names, vec!["notes.md"]);
    }

    #[test]
    fn type_filter_files_only_and_dirs_only() {
        let fs = tree();
        let files = SearchQuery {
            filters: Filters {
                kinds: TypeFilter::FilesOnly,
                ..Default::default()
            },
            ..Default::default()
        };
        let (fnames, _) = hits(&fs, &files);
        assert_eq!(fnames, vec!["a.txt", "b.rs", "c.rs", "d.txt", "notes.md"]);

        let dirs = SearchQuery {
            filters: Filters {
                kinds: TypeFilter::DirsOnly,
                ..Default::default()
            },
            ..Default::default()
        };
        let (dnames, _) = hits(&fs, &dirs);
        assert_eq!(dnames, vec!["deep/", "sub/"]);
    }

    #[test]
    fn ext_filter_restricts_to_files_of_that_extension() {
        let fs = tree();
        let q = SearchQuery {
            filters: Filters {
                ext: Some("txt".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (names, _) = hits(&fs, &q);
        assert_eq!(names, vec!["a.txt", "d.txt"]);
    }

    #[test]
    fn size_filter_bounds_file_bytes() {
        let fs = tree();
        // Byte counts: a.txt=13, b.rs=12, c.rs=13, notes.md=18, d.txt=12.
        // A [14, 18] window uniquely selects the 18-byte notes.md.
        let q = SearchQuery {
            filters: Filters {
                min_size: Some(14),
                max_size: Some(18),
                ..Default::default()
            },
            ..Default::default()
        };
        let (names, _) = hits(&fs, &q);
        assert_eq!(names, vec!["notes.md"]);
    }

    #[test]
    fn mtime_filter_selects_recently_modified() {
        let fs = tree();
        // Age /r/a.txt far into the past; everything else keeps "now".
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs.set_times(Path::new("/r/a.txt"), old, old).unwrap();
        let cutoff = SystemTime::now() - Duration::from_secs(3600);
        let q = SearchQuery {
            name_glob: Some("*.txt".to_string()),
            filters: Filters {
                modified_after: Some(cutoff),
                ..Default::default()
            },
            ..Default::default()
        };
        let (names, _) = hits(&fs, &q);
        // a.txt is too old; d.txt is fresh.
        assert_eq!(names, vec!["d.txt"]);
    }

    #[test]
    fn cancellation_stops_the_walk_and_flags_stats() {
        let fs = tree();
        let compiled = name_query("*").compile().unwrap();
        // Cancel after the very first entry is stat'd.
        let seen = Cell::new(0u64);
        let mut count = 0u64;
        let stats = run_search(
            &fs,
            &compiled,
            Path::new("/r"),
            &|| seen.get() >= 1,
            &mut |_row| {
                count += 1;
                seen.set(seen.get() + 1);
            },
        );
        assert!(stats.cancelled, "cancel must be recorded");
        assert!(count < 7, "cancel must cut the walk short, got {count}");
    }

    #[test]
    fn root_that_is_a_file_yields_nothing_without_panicking() {
        let fs = tree();
        let compiled = name_query("*").compile().unwrap();
        let mut n = 0;
        let stats = run_search(
            &fs,
            &compiled,
            Path::new("/r/a.txt"), // a file, not a dir
            &|| false,
            &mut |_| n += 1,
        );
        assert_eq!(n, 0);
        assert_eq!(stats.scanned, 0);
        assert!(!stats.cancelled);
    }

    #[test]
    fn empty_query_is_not_meaningful() {
        assert!(!SearchQuery::default().is_meaningful());
        assert!(name_query("*.rs").is_meaningful());
        assert!(SearchQuery {
            filters: Filters {
                kinds: TypeFilter::DirsOnly,
                ..Default::default()
            },
            ..Default::default()
        }
        .is_meaningful());
    }

    #[test]
    fn a_bad_glob_is_an_honest_error() {
        let q = name_query("[unterminated");
        assert!(matches!(q.compile(), Err(SearchError::Glob(_))));
    }

    #[test]
    fn a_bad_regex_is_an_honest_error() {
        let q = SearchQuery {
            content: Some(ContentQuery {
                pattern: "(".to_string(),
                mode: ContentMode::Regex,
                case_insensitive: false,
            }),
            ..Default::default()
        };
        assert!(matches!(q.compile(), Err(SearchError::Regex(_))));
    }

    // ── end-to-end over a real temp directory (LiveFileOps + the worker) ────

    struct TempTree(PathBuf);
    impl TempTree {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-files-search-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(base.join("nested")).unwrap();
            std::fs::write(base.join("hit.rs"), b"fn real() {}").unwrap();
            std::fs::write(base.join("skip.md"), b"nope").unwrap();
            std::fs::write(base.join("nested/deep.rs"), b"fn deep() {}").unwrap();
            Self(base)
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// Block-drain a live run until it reports Done (bounded so a bug can't hang
    /// the suite).
    fn drain_to_done(run: &mut SearchRun) -> (Vec<String>, SearchStats) {
        let mut names = Vec::new();
        let mut stats = SearchStats::default();
        for _ in 0..2000 {
            for ev in run.drain() {
                match ev {
                    SearchEvent::Hit(row) => names.push(row.name.clone()),
                    SearchEvent::Done(s) => stats = s,
                }
            }
            if run.finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(run.finished(), "search worker never reported Done");
        names.sort();
        (names, stats)
    }

    #[test]
    fn search_run_streams_real_hits_from_a_real_tree() {
        let t = TempTree::new();
        let q = name_query("*.rs");
        let mut run = SearchRun::spawn(&q, t.0.clone()).expect("spawn");
        let (names, stats) = drain_to_done(&mut run);
        assert_eq!(names, vec!["deep.rs", "hit.rs"]);
        assert_eq!(stats.matched, 2);
        assert!(!stats.cancelled);
    }

    #[test]
    fn search_run_content_grep_over_real_files() {
        let t = TempTree::new();
        let q = SearchQuery {
            content: Some(ContentQuery {
                pattern: "deep".to_string(),
                mode: ContentMode::Substring,
                case_insensitive: true,
            }),
            ..Default::default()
        };
        let mut run = SearchRun::spawn(&q, t.0.clone()).expect("spawn");
        let (names, _) = drain_to_done(&mut run);
        assert_eq!(names, vec!["deep.rs"]);
    }

    #[test]
    fn search_run_cancel_is_honoured() {
        let t = TempTree::new();
        let q = name_query("*");
        let mut run = SearchRun::spawn(&q, t.0.clone()).expect("spawn");
        run.cancel();
        // Even cancelled, the worker still terminates with a Done event.
        let (_names, _stats) = drain_to_done(&mut run);
        assert!(run.is_cancelled());
    }
}
