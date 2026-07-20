//! EDITOR-8 — **project + in-buffer search**: the find/replace bar over the
//! active rope buffer and the project-wide search overlay.
//!
//! Two surfaces, one matcher:
//!
//! * **In-buffer find/replace** ([`FindState`], `Ctrl-F` / `Ctrl-H`): a find bar
//!   over the focused document with case / whole-word / regex toggles, next/prev
//!   cycling, replace-current, and replace-all. Every match is a **char range**
//!   into the live rope, so the panel highlights them in the widget (viewport-
//!   culled, layered over the tree-sitter paint like the LSP diagnostics) and
//!   drives the caret through the existing [`EditorView`](crate::widget::EditorView)
//!   seams — the replace edits mutate the real buffer as undoable steps (§7).
//! * **Project-wide search** ([`ProjectSearch`], `Ctrl-Shift-F`): a query across
//!   the project tree. The honest backend is **ripgrep** (`rg`) when it is on the
//!   `PATH`; absent it, a bounded in-Rust file walk — the same walk shape the
//!   [`crate::finder`] uses (skips `target/` + `.git`, capped files/depth), never
//!   an unbounded crawl. A picked hit opens the file + jumps to the line.
//!
//! The matcher ([`Matcher`] / [`find_matches`]) is pure data-in / data-out (no
//! egui), so the ranking + range logic is unit-tested directly; `show_find` /
//! `show_project` are thin token-styled (§4) renders over that state, mirroring
//! the [`crate::finder`] / [`crate::palette`] overlay idiom (EDITOR-7).

// `module_name_repetitions`: `FindState` / `ProjectSearch` / `MatchOptions` are
// the domain names for this module's public types; trimming the echo of the
// `search` module reads worse. `missing_const_for_fn` (nursery) is over-eager on
// the small mutators — the same allow `finder.rs` / `buffer.rs` make.
#![allow(clippy::module_name_repetitions, clippy::missing_const_for_fn)]

use std::ffi::OsStr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

use mde_egui::egui::{
    self, Align, Align2, Color32, Key, Layout, Modifiers, RichText, ScrollArea, Vec2,
};
use mde_egui::Style;

use crate::tooltip::editor_hover_text;

use regex::{Regex, RegexBuilder};

// ── bounds (mirroring the finder's honest, bounded walk, §7) ─────────────────

/// Hard cap on the files the project walk visits, so a huge tree can't stall the
/// search — a bounded walk (§7), not an unbounded crawl.
const WALK_FILE_CAP: usize = 20_000;
/// Hard cap on recursion depth (belt-and-braces against a very deep / symlink-
/// looped tree; `file_type` treats a symlink as a non-directory, so a symlinked
/// cycle is walked as a leaf, never recursed).
const WALK_DEPTH_CAP: usize = 32;
/// Directory names the walk skips wholesale — build output + VCS metadata.
const SKIP_DIRS: [&str; 2] = ["target", ".git"];
/// Skip files larger than this in the in-Rust walk — a source-tree search reads
/// text files, not multi-megabyte blobs; keeps the fallback bounded.
const FILE_SIZE_CAP: u64 = 2 * 1024 * 1024;
/// Cap on the total hits either backend returns, so a query like `.` over a big
/// tree yields a usable list instead of an unbounded flood.
const HIT_CAP: usize = 500;
/// Max preview length (chars) shown per hit row, so one very long line can't blow
/// the overlay width.
const PREVIEW_MAX: usize = 160;

// ── overlay geometry (§4 spacing units) ──────────────────────────────────────

/// Width of the find bar plate.
const FIND_WIDTH: f32 = Style::SP_XL * 11.0;
/// Width of the find / replace text fields inside the bar.
const FIELD_WIDTH: f32 = Style::SP_XL * 5.0;
/// Width of the project-search overlay plate.
const PROJECT_WIDTH: f32 = Style::SP_XL * 18.0;
/// Max height of the scrolling result list before it scrolls.
const LIST_MAX_H: f32 = Style::SP_XL * 9.0;
/// Vertical drop of the project overlay from the top edge.
const TOP_DROP: f32 = Style::SP_XL * 2.0;
/// Inset of the find bar from the top-right corner.
const FIND_INSET: f32 = Style::SP_M;

// ── match options + the pure matcher ─────────────────────────────────────────

/// The three orthogonal match toggles shared by the in-buffer find and the
/// project search: case sensitivity, whole-word, and regex. Defaults to a plain,
/// case-insensitive literal substring search — the least-surprise default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchOptions {
    /// Match case exactly (default off — case-insensitive, ASCII fold).
    pub case_sensitive: bool,
    /// Require the match to sit on word boundaries (default off).
    pub whole_word: bool,
    /// Treat the query as a regular expression (default off — a literal string).
    pub regex: bool,
}

/// A compiled query: either a literal char-sequence needle or a compiled regex.
///
/// Built once ([`Matcher::new`]) and reused across many haystacks (the project
/// walk scans line-by-line), so a regex compiles a single time, not per line.
enum Matcher {
    /// A literal (non-regex) needle as a char vector, plus the toggles that
    /// affect comparison (case, whole-word).
    Literal {
        /// The query as chars — compared window-by-window against the haystack.
        needle: Vec<char>,
        /// The case / whole-word toggles (regex is always off in this arm).
        opts: MatchOptions,
    },
    /// A compiled regular expression (case + whole-word already folded into it).
    Regex(Regex),
}

impl Matcher {
    /// Compile `query` under `opts`, or `None` for an empty query or an invalid
    /// regex (the honest "no matches" / "bad pattern" state — never a panic).
    fn new(query: &str, opts: MatchOptions) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        if opts.regex {
            compile_regex(query, opts).map(Self::Regex)
        } else {
            Some(Self::Literal {
                needle: query.chars().collect(),
                opts,
            })
        }
    }

    /// Every non-overlapping match of this query in `haystack`, as ascending
    /// **char** ranges (empty matches — e.g. a regex `a*` — are dropped).
    fn find_all(&self, haystack: &str) -> Vec<Range<usize>> {
        match self {
            Self::Literal { needle, opts } => literal_all(haystack, needle, *opts),
            Self::Regex(re) => regex_all(haystack, re),
        }
    }
}

/// Build the effective [`Regex`] for `pattern` under `opts`: case-insensitive
/// unless [`MatchOptions::case_sensitive`], and wrapped in word boundaries when
/// [`MatchOptions::whole_word`]. `None` when the pattern does not compile.
fn compile_regex(pattern: &str, opts: MatchOptions) -> Option<Regex> {
    let effective = if opts.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern.to_owned()
    };
    RegexBuilder::new(&effective)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .ok()
}

/// Every non-overlapping literal match of `needle` in `haystack`, as char ranges.
/// Case folding is ASCII (matching the crate's [`crate::fuzzy`] matcher); a
/// whole-word match additionally requires non-word chars (or the ends) on both
/// sides.
fn literal_all(haystack: &str, needle: &[char], opts: MatchOptions) -> Vec<Range<usize>> {
    let n = needle.len();
    if n == 0 {
        return Vec::new();
    }
    let hay: Vec<char> = haystack.chars().collect();
    if hay.len() < n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + n <= hay.len() {
        let window = &hay[i..i + n];
        if window_matches(window, needle, opts.case_sensitive)
            && (!opts.whole_word || word_bounded(&hay, i, i + n))
        {
            out.push(i..i + n);
            i += n; // non-overlapping
        } else {
            i += 1;
        }
    }
    out
}

/// Whether the `window` of chars equals `needle`, honouring case sensitivity
/// (ASCII fold when insensitive — file text is ASCII-dominant; an exact match
/// still wins on non-ASCII).
fn window_matches(window: &[char], needle: &[char], case_sensitive: bool) -> bool {
    window.iter().zip(needle).all(|(&a, &b)| {
        if case_sensitive {
            a == b
        } else {
            a == b || a.eq_ignore_ascii_case(&b)
        }
    })
}

/// Whether a match spanning chars `start..end` of `hay` sits on word boundaries —
/// the char before it and the char at its end are non-word (alphanumeric / `_`)
/// or the buffer edge.
fn word_bounded(hay: &[char], start: usize, end: usize) -> bool {
    let before_ok = start == 0 || !is_word_char(hay[start - 1]);
    let after_ok = end >= hay.len() || !is_word_char(hay[end]);
    before_ok && after_ok
}

/// Whether `c` is a "word" char for the whole-word toggle (alphanumeric or `_`).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every non-empty regex match of `re` in `haystack`, converted from byte offsets
/// to char ranges (so an astral char advances the byte offset by more than one
/// but the char index by exactly one).
fn regex_all(haystack: &str, re: &Regex) -> Vec<Range<usize>> {
    let byte_ranges: Vec<Range<usize>> = re
        .find_iter(haystack)
        .map(|m| m.start()..m.end())
        .filter(|r| r.end > r.start)
        .collect();
    byte_ranges_to_char_ranges(haystack, &byte_ranges)
}

/// Convert ascending, on-boundary `byte` ranges to char ranges with one pass over
/// `haystack`'s char boundaries (a `partition_point` per endpoint).
fn byte_ranges_to_char_ranges(haystack: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let starts: Vec<usize> = haystack.char_indices().map(|(b, _)| b).collect();
    // The char index of byte offset `byte` = the count of char-starts before it.
    let char_of = |byte: usize| starts.partition_point(|&s| s < byte);
    ranges
        .iter()
        .map(|r| char_of(r.start)..char_of(r.end))
        .collect()
}

/// Every non-overlapping match of `query` (under `opts`) in `haystack`, as
/// ascending char ranges. Empty query / invalid regex yields an empty list. The
/// one seam the widget's highlight ranges, the find bar's cycling, and the
/// project walk's per-line scan all resolve through.
#[must_use]
pub fn find_matches(haystack: &str, query: &str, opts: MatchOptions) -> Vec<Range<usize>> {
    Matcher::new(query, opts).map_or_else(Vec::new, |m| m.find_all(haystack))
}

// ── in-buffer find / replace state (Ctrl-F / Ctrl-H) ─────────────────────────

/// The signal `show_find` returns to the panel for the frame's find bar action —
/// the panel routes each against the live buffer / view (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindEvent {
    /// Nothing happened this frame.
    Idle,
    /// Advance to the next match (Enter / the `↓` button).
    Next,
    /// Step to the previous match (Shift+Enter / the `↑` button).
    Prev,
    /// Replace the current match with the replacement text.
    ReplaceCurrent,
    /// Replace every match with the replacement text.
    ReplaceAll,
}

/// The in-buffer find/replace bar state (EDITOR-8).
///
/// Holds the open flag, whether the replace row is shown (`Ctrl-H` vs `Ctrl-F`),
/// the query + replacement, the toggles, and the resolved matches + current index.
/// The panel [`recompute`](FindState::recompute)s the matches each frame the bar
/// is open (against the live rope), highlights them in the widget, and drives the
/// caret / edits from the [`FindEvent`] `show_find` returns.
///
// `struct_excessive_bools`: the find bar's flags (shown / replace-row-shown /
// regex-error / focus-grab) are genuinely independent one-shot UI states, not a
// state enum in disguise — a bitset would read worse than the named fields.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub struct FindState {
    /// Whether the find bar is shown.
    open: bool,
    /// Whether the replace row is shown (`Ctrl-H`) as well as the find row.
    replace_shown: bool,
    /// The live find query.
    query: String,
    /// The live replacement text.
    replacement: String,
    /// The case / whole-word / regex toggles.
    opts: MatchOptions,
    /// The resolved matches — ascending char ranges into the active buffer,
    /// rebuilt by [`recompute`](Self::recompute).
    matches: Vec<Range<usize>>,
    /// The current match — an index into `matches`.
    current: usize,
    /// Whether the last recompute saw an invalid regex (the honest "bad pattern"
    /// note, never a fake empty result masquerading as "no matches").
    regex_error: bool,
    /// Set on open so the query field grabs the keyboard for one frame.
    focus_query: bool,
    /// The count from the last Replace All, shown until the query changes.
    last_replaced: Option<usize>,
}

impl FindState {
    /// Whether the find bar is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the find bar (`Ctrl-F`) — find row only, focus the query.
    pub fn open_find(&mut self) {
        self.open = true;
        self.replace_shown = false;
        self.focus_query = true;
        self.last_replaced = None;
    }

    /// Open the find + replace bar (`Ctrl-H`) — both rows, focus the query.
    pub fn open_replace(&mut self) {
        self.open = true;
        self.replace_shown = true;
        self.focus_query = true;
        self.last_replaced = None;
    }

    /// Close the bar (Esc). Keeps the query so re-opening resumes the search.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Rebuild the matches from `text` (the active buffer's contents) under the
    /// current query + toggles, clamping the current index. Called each frame the
    /// bar is open, so the highlight + counter track edits and toggle flips.
    pub fn recompute(&mut self, text: &str) {
        self.matches = find_matches(text, &self.query, self.opts);
        self.regex_error = self.opts.regex
            && !self.query.is_empty()
            && Matcher::new(&self.query, self.opts).is_none();
        if self.current >= self.matches.len() {
            self.current = self.matches.len().saturating_sub(1);
        }
    }

    /// Drop the resolved matches (the active document closed / has no buffer).
    pub fn clear_matches(&mut self) {
        self.matches.clear();
        self.current = 0;
        self.regex_error = false;
    }

    /// The resolved matches — the widget's highlight ranges.
    #[must_use]
    pub fn matches(&self) -> &[Range<usize>] {
        &self.matches
    }

    /// The current match index (clamped into `matches`).
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.current.min(self.matches.len().saturating_sub(1))
    }

    /// The current match's char range, or `None` when there are no matches — what
    /// next/prev reveals and Replace mutates.
    #[must_use]
    pub fn current_range(&self) -> Option<Range<usize>> {
        self.matches.get(self.current_index()).cloned()
    }

    /// Step the current match one step (wrapping, the find idiom).
    pub fn cycle(&mut self, forward: bool) {
        let len = self.matches.len();
        if len == 0 {
            return;
        }
        self.current = if forward {
            (self.current_index() + 1) % len
        } else {
            (self.current_index() + len - 1) % len
        };
    }

    /// The replacement text (Replace / Replace All source).
    #[must_use]
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Record the count from a Replace All (shown until the query changes).
    pub fn set_last_replaced(&mut self, n: usize) {
        self.last_replaced = Some(n);
    }

    /// Whether the last recompute saw an invalid regex.
    #[must_use]
    pub const fn regex_error(&self) -> bool {
        self.regex_error
    }

    // ── test seams (drive the state without the egui field) ──────────────────

    /// Set the query directly — the test seam (the live path writes the egui
    /// field; the panel's tests drive the state through this).
    #[cfg(test)]
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
        self.last_replaced = None;
    }

    /// Set the replacement text directly — the test seam.
    #[cfg(test)]
    pub fn set_replacement(&mut self, replacement: &str) {
        self.replacement = replacement.to_owned();
    }
}

// ── project-wide search state (Ctrl-Shift-F) ─────────────────────────────────

/// Which backend produced a project-search result set — surfaced in the overlay
/// so the operator knows whether the honest `rg` ran or the in-Rust fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Shelled out to `ripgrep` (`rg` on the `PATH`).
    Ripgrep,
    /// The bounded in-Rust file walk (no `rg` found).
    Walk,
}

impl Backend {
    /// The short label shown in the overlay chrome.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ripgrep => "ripgrep",
            Self::Walk => "built-in walk",
        }
    }
}

/// One project-search hit: the file, the 1-based line + column, and the trimmed
/// line preview shown in the results list.
#[derive(Clone, Debug)]
pub struct Hit {
    /// The file the match is in.
    pub path: PathBuf,
    /// The 1-based line number.
    pub line: usize,
    /// The 1-based column (char offset + 1 in the walk; `rg`'s reported column).
    pub col: usize,
    /// The trimmed, length-capped line preview.
    pub preview: String,
}

/// The project-wide search overlay state (EDITOR-8).
///
/// Holds the query + toggles, the root to search, the last result set + which
/// backend produced it, and the highlighted row. The search runs on submit (Enter
/// / the Search button) — not per keystroke, since it shells `rg` or walks the
/// tree — so [`needs_run`](Self::needs_run) gates a re-run on the query changing.
#[derive(Default)]
pub struct ProjectSearch {
    /// Whether the overlay is shown.
    open: bool,
    /// The live query.
    query: String,
    /// The case / whole-word / regex toggles.
    opts: MatchOptions,
    /// The project root the search walks / `rg`s.
    root: PathBuf,
    /// The last result set.
    results: Vec<Hit>,
    /// The highlighted row (index into `results`).
    selected: usize,
    /// Which backend produced `results` (once a search has run).
    backend: Option<Backend>,
    /// The query `results` reflect, so [`needs_run`](Self::needs_run) can tell a
    /// stale result set from a current one.
    ran_query: Option<String>,
    /// Set on open so the query field grabs the keyboard for one frame.
    focus_query: bool,
}

impl ProjectSearch {
    /// Whether the overlay is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overlay rooted at `root`, clearing any stale results.
    pub fn open_at(&mut self, root: PathBuf) {
        self.open = true;
        self.root = root;
        self.results.clear();
        self.backend = None;
        self.ran_query = None;
        self.selected = 0;
        self.focus_query = true;
    }

    /// Close the overlay (Esc, or after a pick).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The live query — the panel reads it to jump into an opened hit exactly.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Set the query directly — the test seam (the live path writes the egui
    /// field).
    #[cfg(test)]
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
    }

    /// The match toggles.
    #[must_use]
    pub const fn options(&self) -> MatchOptions {
        self.opts
    }

    /// Whether the current query differs from the one `results` were run for — so
    /// Enter re-runs the search instead of opening a stale hit.
    #[must_use]
    pub fn needs_run(&self) -> bool {
        self.ran_query.as_deref() != Some(self.query.as_str())
    }

    /// Run the search over the root (rg if present, else the bounded walk),
    /// replacing the result set + recording the backend.
    pub fn run(&mut self) {
        let (results, backend) = search_project(&self.root, &self.query, self.opts);
        self.results = results;
        self.backend = Some(backend);
        self.ran_query = Some(self.query.clone());
        self.selected = 0;
    }

    /// Move the highlighted row within the results (saturating; the list doesn't
    /// wrap).
    fn move_selection(&mut self, forward: bool) {
        let len = self.results.len();
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

    /// The highlighted hit, if any — what Enter opens.
    #[must_use]
    fn selected_hit(&self) -> Option<&Hit> {
        self.results.get(self.selected)
    }

    /// The result set — the test seam for asserting a run's hits (the live path
    /// returns a picked [`Hit`] from [`show_project`] instead of enumerating).
    #[cfg(test)]
    #[must_use]
    pub fn results(&self) -> &[Hit] {
        &self.results
    }

    /// The backend that produced the current results — the test seam.
    #[cfg(test)]
    #[must_use]
    pub const fn backend(&self) -> Option<Backend> {
        self.backend
    }
}

// ── the two project-search backends ──────────────────────────────────────────

/// Search `root` for `query` under `opts`: the honest `rg` when it is on the
/// `PATH`, else the bounded in-Rust walk. Returns the hits + which backend ran.
#[must_use]
pub fn search_project(root: &Path, query: &str, opts: MatchOptions) -> (Vec<Hit>, Backend) {
    if query.is_empty() {
        return (Vec::new(), Backend::Walk);
    }
    // ripgrep is the honest tool when it is on the PATH; absent it (a spawn
    // failure), fall through to the bounded in-Rust walk.
    if let Some(hits) = search_ripgrep(root, query, opts) {
        return (hits, Backend::Ripgrep);
    }
    (search_walk(root, query, opts), Backend::Walk)
}

/// Shell `rg` for `query` under `root`, parsing its `path:line:col:preview` lines.
/// `None` when `rg` is not installed (the spawn fails) or errored — the caller
/// then falls back to the in-Rust walk. An `rg` that runs but finds nothing
/// returns `Some(empty)` (a real "no matches", not a fallback trigger).
fn search_ripgrep(root: &Path, query: &str, opts: MatchOptions) -> Option<Vec<Hit>> {
    let mut cmd = Command::new("rg");
    cmd.arg("--line-number")
        .arg("--column")
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        .arg("--max-count")
        .arg(HIT_CAP.to_string());
    if !opts.regex {
        cmd.arg("--fixed-strings");
    }
    if opts.whole_word {
        cmd.arg("--word-regexp");
    }
    if opts.case_sensitive {
        cmd.arg("--case-sensitive");
    } else {
        cmd.arg("--ignore-case");
    }
    cmd.arg("--").arg(query).arg(root);

    let output = cmd.output().ok()?; // None ⇒ `rg` not found ⇒ fall back to the walk
                                     // rg exits 0 (matches), 1 (no matches), 2 (error). Treat a hard error as
                                     // "fall back to the walk"; 0/1 parse the (possibly empty) stdout.
    if output.status.code() == Some(2) {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut hits = Vec::new();
    for line in text.lines() {
        if let Some(hit) = parse_rg_line(line) {
            hits.push(hit);
        }
        if hits.len() >= HIT_CAP {
            break;
        }
    }
    Some(hits)
}

/// Parse one `rg --column` line (`path:line:col:preview`) into a [`Hit`], or
/// `None` for a malformed line.
fn parse_rg_line(line: &str) -> Option<Hit> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?;
    let lnum: usize = parts.next()?.parse().ok()?;
    let col: usize = parts.next()?.parse().ok()?;
    let preview = truncate_preview(parts.next().unwrap_or("").trim());
    Some(Hit {
        path: PathBuf::from(path),
        line: lnum,
        col,
        preview,
    })
}

/// The bounded in-Rust fallback: walk `root` (skipping `target/` + `.git`, capped
/// files/depth), read each UTF-8 text file, and scan its lines for `query` under
/// `opts` — the honest tool when `rg` is absent (§7).
#[must_use]
pub fn search_walk(root: &Path, query: &str, opts: MatchOptions) -> Vec<Hit> {
    let Some(matcher) = Matcher::new(query, opts) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    walk_into(root, 0, &mut files);
    let mut hits = Vec::new();
    'files: for path in files {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > FILE_SIZE_CAP {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // Skip non-UTF-8 (binary) files rather than lossily "matching" bytes.
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for r in matcher.find_all(line) {
                hits.push(Hit {
                    path: path.clone(),
                    line: i + 1,
                    col: r.start + 1,
                    preview: truncate_preview(line.trim()),
                });
                if hits.len() >= HIT_CAP {
                    break 'files;
                }
            }
        }
    }
    hits
}

/// Recurse one directory into `files`, honouring the file + depth caps and
/// skipping `target/` + `.git` — the same bounded, panic-free shape the finder's
/// walk uses (an unreadable directory is skipped, never a panic; §7).
fn walk_into(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > WALK_DEPTH_CAP || files.len() >= WALK_FILE_CAP {
        return;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
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

/// Trim a preview to [`PREVIEW_MAX`] chars, appending an ellipsis when clipped.
fn truncate_preview(line: &str) -> String {
    let mut out: String = line.chars().take(PREVIEW_MAX).collect();
    if line.chars().count() > PREVIEW_MAX {
        out.push('\u{2026}');
    }
    out
}

// ── overlays (thin token-styled renders, mirroring the finder idiom) ─────────

/// Render the in-buffer find/replace bar on `ctx` and return the frame's
/// [`FindEvent`] (Idle when nothing happened). A no-op returning `Idle` while the
/// bar is closed. The nav chords (Enter / Shift+Enter / Esc) are consumed here —
/// before the text fields read them — so Enter cycles matches and Esc dismisses.
#[allow(clippy::too_many_lines)]
pub fn show_find(ctx: &egui::Context, find: &mut FindState) -> FindEvent {
    if !find.open {
        return FindEvent::Idle;
    }
    let (enter, shift_enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::SHIFT, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        find.close();
        return FindEvent::Idle;
    }
    let mut event = if shift_enter {
        FindEvent::Prev
    } else if enter {
        FindEvent::Next
    } else {
        FindEvent::Idle
    };

    egui::Window::new("Find in file")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::RIGHT_TOP, Vec2::new(-FIND_INSET, FIND_INSET))
        .show(ctx, |ui| {
            ui.set_min_width(FIND_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Find")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .strong(),
                );
                let field = ui.add(
                    egui::TextEdit::singleline(&mut find.query)
                        .hint_text("Find in file\u{2026}")
                        .desired_width(FIELD_WIDTH),
                );
                if std::mem::take(&mut find.focus_query) {
                    field.request_focus();
                }
                if field.changed() {
                    find.last_replaced = None;
                }
                // `|=` (not `||`) so every toggle renders regardless of the first.
                let mut toggled = toggle(ui, &mut find.opts.case_sensitive, "Aa", "Match case");
                toggled |= toggle(ui, &mut find.opts.whole_word, "W", "Whole word");
                toggled |= toggle(ui, &mut find.opts.regex, ".*", "Regular expression");
                if toggled {
                    find.last_replaced = None;
                }
                if editor_hover_text(
                    ui.button(RichText::new("\u{2191}").size(Style::SMALL)),
                    "Previous (Shift+Enter)",
                )
                .clicked()
                {
                    event = FindEvent::Prev;
                }
                if editor_hover_text(
                    ui.button(RichText::new("\u{2193}").size(Style::SMALL)),
                    "Next (Enter)",
                )
                .clicked()
                {
                    event = FindEvent::Next;
                }
            });

            let (counter, color) = counter_label(find);
            if !counter.is_empty() {
                ui.label(RichText::new(counter).size(Style::SMALL).color(color));
            }

            if find.replace_shown {
                ui.add_space(Style::SP_XS);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Repl")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM)
                            .strong(),
                    );
                    let rf = ui.add(
                        egui::TextEdit::singleline(&mut find.replacement)
                            .hint_text("Replace with\u{2026}")
                            .desired_width(FIELD_WIDTH),
                    );
                    if rf.changed() {
                        find.last_replaced = None;
                    }
                    if editor_hover_text(
                        ui.button(RichText::new("Replace").size(Style::SMALL)),
                        "Replace the current match",
                    )
                    .clicked()
                    {
                        event = FindEvent::ReplaceCurrent;
                    }
                    if editor_hover_text(
                        ui.button(RichText::new("All").size(Style::SMALL)),
                        "Replace every match",
                    )
                    .clicked()
                    {
                        event = FindEvent::ReplaceAll;
                    }
                });
                if let Some(n) = find.last_replaced {
                    ui.label(
                        RichText::new(format!("Replaced {n}"))
                            .size(Style::SMALL)
                            .color(Style::OK),
                    );
                }
            }
        });

    event
}

/// The find bar's match-counter line + its tone: the honest "bad pattern" note
/// for an invalid regex, "No results" for a live-but-unmatched query, or
/// "`n` of `m`" for the current position.
fn counter_label(find: &FindState) -> (String, Color32) {
    if find.regex_error() {
        return ("Invalid pattern".to_owned(), Style::WARN);
    }
    if find.query.is_empty() {
        return (String::new(), Style::TEXT_DIM);
    }
    if find.matches.is_empty() {
        return ("No results".to_owned(), Style::TEXT_DIM);
    }
    (
        format!("{} of {}", find.current_index() + 1, find.matches.len()),
        Style::TEXT_DIM,
    )
}

/// One toggle chip bound to `flag`; returns `true` when it was clicked this frame.
fn toggle(ui: &mut egui::Ui, flag: &mut bool, label: &str, hover: &str) -> bool {
    let clicked = editor_hover_text(
        ui.selectable_label(*flag, RichText::new(label).size(Style::SMALL)),
        hover,
    )
    .clicked();
    if clicked {
        *flag = !*flag;
    }
    clicked
}

/// Render the project-wide search overlay on `ctx` and return the [`Hit`] the
/// operator opened this frame (Enter on the highlighted row after a search has
/// run, or a click), closing the overlay on a pick or Esc. Enter with a changed
/// query runs the search instead of opening. A no-op returning `None` while the
/// overlay is closed.
#[allow(clippy::too_many_lines)]
pub fn show_project(ctx: &egui::Context, ps: &mut ProjectSearch) -> Option<Hit> {
    if !ps.open {
        return None;
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
        ps.close();
        return None;
    }
    if up {
        ps.move_selection(false);
    }
    if down {
        ps.move_selection(true);
    }
    let mut picked: Option<Hit> = None;
    if enter {
        if ps.needs_run() {
            ps.run();
        } else {
            picked = ps.selected_hit().cloned();
        }
    }
    // A row click (resolved after the window renders so it doesn't clash with the
    // immutable borrow the results list holds).
    let mut clicked_row: Option<usize> = None;

    egui::Window::new("Search in project")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_DROP))
        .show(ctx, |ui| {
            ui.set_min_width(PROJECT_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Search project")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .strong(),
                );
                if let Some(backend) = ps.backend {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(backend.label())
                                .size(Style::SMALL)
                                .color(Style::TEXT_DIM),
                        );
                    });
                }
            });
            ui.add_space(Style::SP_XS);

            let field = ui.add(
                egui::TextEdit::singleline(&mut ps.query)
                    .hint_text("Type a query, Enter to search\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut ps.focus_query) {
                field.request_focus();
            }
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                toggle(ui, &mut ps.opts.case_sensitive, "Aa", "Match case");
                toggle(ui, &mut ps.opts.whole_word, "W", "Whole word");
                toggle(ui, &mut ps.opts.regex, ".*", "Regular expression");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(RichText::new("Search").size(Style::SMALL))
                        .clicked()
                    {
                        ps.run();
                    }
                });
            });
            ui.add_space(Style::SP_XS);
            ui.separator();

            if ps.results.is_empty() {
                ui.add_space(Style::SP_XS);
                let msg = if ps.ran_query.is_some() {
                    "No matches in the project"
                } else {
                    "Enter a query to search the project"
                };
                ui.label(
                    RichText::new(msg)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .italics(),
                );
                return;
            }

            ScrollArea::vertical()
                .id_salt("editor-project-search-list")
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (row, hit) in ps.results.iter().enumerate() {
                        if hit_row(ui, hit, row == ps.selected) {
                            clicked_row = Some(row);
                        }
                    }
                });
        });

    if let Some(row) = clicked_row {
        ps.selected = row;
        picked = ps.results.get(row).cloned();
    }
    if picked.is_some() {
        ps.close();
    }
    picked
}

/// One project-search result row: `file:line:col  preview`, highlighted when
/// selected. Returns `true` when clicked. Token-styled (§4).
fn hit_row(ui: &mut egui::Ui, hit: &Hit, selected: bool) -> bool {
    let name = hit.path.file_name().map_or_else(
        || hit.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let label = format!("{}:{}:{}  {}", name, hit.line, hit.col, hit.preview);
    ui.selectable_label(
        selected,
        RichText::new(label).size(Style::SMALL).color(Style::TEXT),
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::{
        find_matches, search_project, search_walk, Backend, FindState, MatchOptions, Matcher,
        ProjectSearch,
    };
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique temp dir for a live search test, cleaned up on drop (the same
    /// idiom the finder / project-tree tests use — no `tempfile` GUI dep).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-editor-search-{tag}-{}-{}",
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

    // ── the pure matcher ─────────────────────────────────────────────────────

    #[test]
    fn literal_find_returns_every_non_overlapping_match() {
        // "the cat sat" — "at" at chars 5..7 (c-a-t) and 9..11 (s-a-t).
        let m = find_matches("the cat sat", "at", MatchOptions::default());
        assert_eq!(m, vec![5..7, 9..11], "both 'at' runs, non-overlapping");
        assert_eq!(
            find_matches("the cat sat", "at", MatchOptions::default()).len(),
            2
        );
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(find_matches("anything", "", MatchOptions::default()).is_empty());
    }

    #[test]
    fn case_toggle_narrows_the_matches() {
        let ci = find_matches("Foo foo", "foo", MatchOptions::default());
        assert_eq!(ci.len(), 2, "case-insensitive matches Foo and foo");
        let cs = find_matches(
            "Foo foo",
            "foo",
            MatchOptions {
                case_sensitive: true,
                ..MatchOptions::default()
            },
        );
        assert_eq!(cs, vec![4..7], "case-sensitive matches only lowercase foo");
    }

    #[test]
    fn whole_word_toggle_rejects_substring_hits() {
        let opts = MatchOptions {
            whole_word: true,
            ..MatchOptions::default()
        };
        // "cat category" — only the standalone "cat" (0..3) is word-bounded.
        assert_eq!(find_matches("cat category", "cat", opts), vec![0..3]);
        // Without the toggle, the "cat" inside "category" also matches.
        assert_eq!(
            find_matches("cat category", "cat", MatchOptions::default()).len(),
            2
        );
    }

    #[test]
    fn regex_toggle_matches_a_pattern() {
        let opts = MatchOptions {
            regex: true,
            ..MatchOptions::default()
        };
        // "a1 b2 c3" — \d hits the three digits at chars 1, 4, 7.
        let m = find_matches("a1 b2 c3", r"\d", opts);
        assert_eq!(m, vec![1..2, 4..5, 7..8]);
    }

    #[test]
    fn regex_match_maps_byte_offsets_to_char_ranges() {
        // "café x1": é is two UTF-8 bytes, so the digit's byte offset (7) resolves
        // to char index 6 — the conversion is char-correct, not byte-correct.
        let opts = MatchOptions {
            regex: true,
            ..MatchOptions::default()
        };
        let m = find_matches("café x1", r"\d", opts);
        assert_eq!(
            m,
            vec![6..7],
            "the digit lands on char 6, past the 2-byte é"
        );
    }

    #[test]
    fn invalid_regex_yields_no_matches_and_no_matcher() {
        let opts = MatchOptions {
            regex: true,
            ..MatchOptions::default()
        };
        assert!(
            find_matches("a(b", "(", opts).is_empty(),
            "an unclosed group matches nothing"
        );
        assert!(
            Matcher::new("(", opts).is_none(),
            "an invalid regex compiles to no matcher"
        );
    }

    // ── FindState ────────────────────────────────────────────────────────────

    #[test]
    fn find_state_recomputes_cycles_and_flags_bad_regex() {
        let mut find = FindState::default();
        find.open_find();
        assert!(find.is_open());
        find.set_query("ab");
        find.recompute("ab cab ab");
        // "ab cab ab": chars a0 b1 sp2 c3 a4 b5 sp6 a7 b8 → "ab" at 0..2, 4..6, 7..9.
        assert_eq!(find.matches(), &[0..2, 4..6, 7..9]);
        assert_eq!(
            find.current_range(),
            Some(0..2),
            "starts at the first match"
        );
        find.cycle(true);
        assert_eq!(find.current_range(), Some(4..6), "next advances");
        find.cycle(false);
        find.cycle(false);
        assert_eq!(
            find.current_range(),
            Some(7..9),
            "prev wraps past the start"
        );

        // Flip to regex with a broken pattern → the honest error flag, no matches.
        find.opts = MatchOptions {
            regex: true,
            ..MatchOptions::default()
        };
        find.set_query("(");
        find.recompute("ab cab ab");
        assert!(find.regex_error(), "an invalid regex sets the error flag");
        assert!(find.matches().is_empty());
    }

    // ── project search ───────────────────────────────────────────────────────

    #[test]
    fn walk_finds_real_hits_and_skips_target_and_git() {
        let d = TempDir::new("walk");
        std::fs::create_dir(d.join("src")).expect("mkdir src");
        std::fs::write(d.join("src/lib.rs"), b"fn alpha() {}\nfn beta() {}\n").expect("write lib");
        std::fs::write(d.join("src/mod.rs"), b"// beta lives here\n").expect("write mod");
        // Noise the walk must skip.
        std::fs::create_dir(d.join("target")).expect("mkdir target");
        std::fs::write(d.join("target/gen.rs"), b"fn beta() {}\n").expect("write gen");
        std::fs::create_dir(d.join(".git")).expect("mkdir .git");
        std::fs::write(d.join(".git/COMMIT"), b"beta\n").expect("write git");

        let hits = search_walk(&d.0, "beta", MatchOptions::default());
        // Two real hits (lib.rs line 2, mod.rs line 1); nothing under target/.git.
        assert_eq!(hits.len(), 2, "only the source-tree hits: {hits:?}");
        assert!(
            hits.iter()
                .all(|h| !h.path.starts_with(d.join("target"))
                    && !h.path.starts_with(d.join(".git"))),
            "target/ and .git/ are skipped"
        );
        let lib_hit = hits
            .iter()
            .find(|h| h.path.ends_with("lib.rs"))
            .expect("a hit in lib.rs");
        assert_eq!(lib_hit.line, 2, "beta is on line 2 of lib.rs");
        assert!(
            lib_hit.preview.contains("beta"),
            "the preview carries the matched line: {}",
            lib_hit.preview
        );
    }

    #[test]
    fn walk_column_is_one_based_char_offset() {
        let d = TempDir::new("col");
        std::fs::write(d.join("a.txt"), b"xx needle\n").expect("write");
        let hits = search_walk(&d.0, "needle", MatchOptions::default());
        let hit = hits.first().expect("one hit");
        assert_eq!(hit.line, 1);
        assert_eq!(hit.col, 4, "needle starts at char 3 → 1-based col 4");
    }

    #[test]
    fn search_project_returns_hits_via_whichever_backend() {
        // Backend-agnostic: whether rg is installed on the host or not, the search
        // resolves the seeded match (rg when present, the walk fallback otherwise).
        let d = TempDir::new("project");
        std::fs::write(d.join("hello.rs"), b"fn hello() { greet(); }\n").expect("write");
        let (hits, backend) = search_project(&d.0, "greet", MatchOptions::default());
        assert!(!hits.is_empty(), "the seeded match is found ({backend:?})");
        assert!(
            hits.iter().any(|h| h.path.ends_with("hello.rs")),
            "the hit points at the seeded file"
        );
        assert!(matches!(backend, Backend::Ripgrep | Backend::Walk));
    }

    #[test]
    fn project_search_state_runs_and_gates_reruns() {
        let d = TempDir::new("ps-state");
        std::fs::write(d.join("f.rs"), b"let value = 1;\n").expect("write");
        let mut ps = ProjectSearch::default();
        ps.open_at(d.0.clone());
        assert!(ps.is_open());
        ps.query = "value".to_owned();
        assert!(ps.needs_run(), "a fresh query needs a run");
        ps.run();
        assert!(!ps.needs_run(), "after running, the query is current");
        assert!(!ps.results().is_empty(), "the run found the seeded match");
        assert!(ps.backend().is_some(), "the run recorded its backend");
    }
}
