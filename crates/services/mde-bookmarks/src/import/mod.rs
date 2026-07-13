//! Browser bookmark importers (BOOKMARKS-3; design: `docs/design/mesh-bookmarks.md`).
//!
//! Import bookmarks from any major browser into the mesh collection: Firefox
//! `places.sqlite` (bookmarks only, read-only + immutable), Chromium `Bookmarks`
//! JSON, and the universal Netscape bookmark-HTML (Safari via its HTML export).
//! Each format parser is pure `bytes/path -> ParsedTree` ([`firefox`],
//! [`chromium`], [`netscape`]); [`plan_import`] is the sole CRDT glue, folding a
//! parsed tree into the model's ops under `Imported/<Browser>` with
//! normalized-URL dedup + idempotent re-import (§6 glue-not-reimplementation).
//!
//! **Security lock (operator): NEVER read logins, cookies, saved passwords, or
//! history — bookmarks ONLY.** See [`firefox`] for how the Firefox importer
//! enforces this (immutable open, bookmark-tables-only queries, schema backstop).
//! The Chromium/Netscape formats carry no credential data at all.

mod chromium;
mod firefox;
// `pub(crate)` (not `pub`) so the exporter (`crate::export`) can call
// `netscape::parse` directly in its round-trip tests, without widening the
// crate's public API — the format parser stays an import-only implementation
// detail to the outside world.
pub(crate) mod netscape;
mod normalize;
mod parsed;
mod plan;

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::{Author, Collection, HlcClock, Source};

pub use normalize::normalize_url;
pub use parsed::{ParsedBookmark, ParsedNode, ParsedTree};
pub use plan::{plan_import, ImportOutcome};

/// A detected bookmark file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    /// Firefox `places.sqlite` (opened read-only + immutable; bookmarks only).
    FirefoxSqlite,
    /// A Chromium/Chrome/Edge/Brave `Bookmarks` JSON file.
    ChromiumJson,
    /// A universal Netscape bookmark-HTML export (incl. Safari).
    NetscapeHtml,
}

impl ImportFormat {
    /// The default browser [`Source`] for this format.
    #[must_use]
    pub const fn default_source(self) -> Source {
        match self {
            Self::FirefoxSqlite => Source::Firefox,
            Self::ChromiumJson => Source::Chromium,
            Self::NetscapeHtml => Source::NetscapeHtml,
        }
    }
}

/// A picker candidate: an importable file found under a scanned directory
/// (lock Q9 — "if a dir holds multiple profiles, list them").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    /// The file to import.
    pub path: PathBuf,
    /// Its detected format.
    pub format: ImportFormat,
    /// A short human label (the containing profile/dir name).
    pub label: String,
}

/// An import failure. Typed (§9 — no shell, no stringly errors at the boundary).
#[derive(Debug)]
pub enum ImportError {
    /// A filesystem error reading the source.
    Io(io::Error),
    /// The file's format could not be recognized.
    UnknownFormat,
    /// A `SQLite` file was opened but lacks the Firefox bookmark tables (e.g. a
    /// misdetected `cookies.sqlite`) — refused before any content query.
    MissingBookmarkTables,
    /// A `rusqlite` error while reading `places.sqlite`.
    Sqlite(rusqlite::Error),
    /// A JSON parse error reading a Chromium `Bookmarks` file.
    Json(serde_json::Error),
    /// A structural problem in an otherwise-parsed file.
    Malformed(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading bookmark file: {e}"),
            Self::UnknownFormat => write!(f, "unrecognized bookmark file format"),
            Self::MissingBookmarkTables => {
                write!(
                    f,
                    "SQLite file has no Firefox bookmark tables (not a places.sqlite)"
                )
            }
            Self::Sqlite(e) => write!(f, "reading places.sqlite: {e}"),
            Self::Json(e) => write!(f, "parsing Chromium bookmarks JSON: {e}"),
            Self::Malformed(m) => write!(f, "malformed bookmark file: {m}"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Sqlite(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::UnknownFormat | Self::MissingBookmarkTables | Self::Malformed(_) => None,
        }
    }
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for ImportError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<serde_json::Error> for ImportError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Auto-detect a file's bookmark format from its leading bytes (lock Q9).
///
/// Never opens the file as a database and never reads credential files — it only
/// sniffs a small header. A `SQLite` magic yields [`ImportFormat::FirefoxSqlite`];
/// the Firefox importer's schema check is the backstop that refuses a non-places
/// `SQLite` before reading any content.
///
/// # Errors
/// Returns an [`io::Error`] if the file cannot be opened or read.
pub fn detect_format(path: &Path) -> io::Result<Option<ImportFormat>> {
    let mut head = [0u8; 4096];
    let read = fs::File::open(path)?.read(&mut head)?;
    Ok(sniff(&head[..read]))
}

/// Classify a header byte slice.
fn sniff(bytes: &[u8]) -> Option<ImportFormat> {
    const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";
    if bytes.starts_with(SQLITE_MAGIC) {
        return Some(ImportFormat::FirefoxSqlite);
    }
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    if lower.contains("netscape-bookmark-file") {
        return Some(ImportFormat::NetscapeHtml);
    }
    if text.trim_start().starts_with('{') {
        return Some(ImportFormat::ChromiumJson);
    }
    if lower.contains("<dl") || lower.contains("<a href") {
        return Some(ImportFormat::NetscapeHtml);
    }
    None
}

/// Parse a file into a [`ParsedTree`], auto-detecting its format.
///
/// # Errors
/// Returns [`ImportError`] if the format is unrecognized or the file cannot be
/// read or parsed.
pub fn parse_file(path: &Path) -> Result<ParsedTree, ImportError> {
    match detect_format(path)?.ok_or(ImportError::UnknownFormat)? {
        ImportFormat::FirefoxSqlite => firefox::parse_places(path),
        ImportFormat::ChromiumJson => chromium::parse(&fs::read(path)?),
        ImportFormat::NetscapeHtml => Ok(netscape::parse(&fs::read_to_string(path)?)),
    }
}

/// Import a picked file into `collection`, auto-detecting its format (locks Q9–Q16).
///
/// Everything lands under `Imported/<Browser>`; returns the ops to apply plus a
/// summary. The collection is not mutated.
///
/// # Errors
/// Returns [`ImportError`] if the file cannot be read, its format is
/// unrecognized, or the source cannot be parsed.
pub fn import_file(
    collection: &Collection,
    path: &Path,
    clock: &mut HlcClock,
    author: &Author,
    now_ms: u64,
) -> Result<ImportOutcome, ImportError> {
    let tree = parse_file(path)?;
    Ok(plan_import(collection, &tree, clock, author, now_ms))
}

/// Like [`import_file`] but forcing the `<Browser>` label (e.g. a Netscape HTML
/// file known to be a Safari export lands under `Imported/Safari`).
///
/// # Errors
/// Returns [`ImportError`] on the same conditions as [`import_file`].
pub fn import_file_as(
    collection: &Collection,
    path: &Path,
    browser: Source,
    clock: &mut HlcClock,
    author: &Author,
    now_ms: u64,
) -> Result<ImportOutcome, ImportError> {
    let mut tree = parse_file(path)?;
    tree.source = browser;
    Ok(plan_import(collection, &tree, clock, author, now_ms))
}

/// List importable bookmark files at `path` (a file, or a directory holding one
/// or more browser profiles) for the picker (lock Q9).
///
/// Only files whose *name* is a plausible bookmark export are sniffed
/// (`places.sqlite`, `Bookmarks`, `*.html`/`*.htm`) — credential files like
/// `cookies.sqlite` / `logins.json` / `key4.db` are never touched. Scans the
/// directory and one level of subdirectories (browser profile dirs).
///
/// # Errors
/// Returns an [`io::Error`] if `path` cannot be stat-ed or a directory read.
pub fn scan_profiles(path: &Path) -> io::Result<Vec<ImportCandidate>> {
    if fs::metadata(path)?.is_file() {
        return Ok(detect_format(path)?
            .map(|format| vec![candidate(path, format)])
            .unwrap_or_default());
    }
    let mut out = Vec::new();
    collect_candidates(path, &mut out)?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_candidates(&entry.path(), &mut out)?;
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup();
    Ok(out)
}

/// Sniff the plausibly-named files directly in `dir`, appending candidates.
fn collect_candidates(dir: &Path, out: &mut Vec<ImportCandidate>) -> io::Result<()> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file() && is_candidate_name(&path) {
            if let Some(format) = detect_format(&path)? {
                out.push(candidate(&path, format));
            }
        }
    }
    Ok(())
}

/// Whether a file name is a plausible bookmark export (never a credential file).
fn is_candidate_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_html = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"));
    name == "places.sqlite" || name == "bookmarks" || name == "bookmarks.json" || is_html
}

/// Build a candidate with a label taken from the containing directory name.
fn candidate(path: &Path, format: ImportFormat) -> ImportCandidate {
    let label = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    ImportCandidate {
        path: path.to_path_buf(),
        format,
        label,
    }
}

#[cfg(test)]
mod tests {
    use super::{sniff, ImportFormat};

    #[test]
    fn sniff_detects_each_format() {
        assert_eq!(
            sniff(b"SQLite format 3\0rest"),
            Some(ImportFormat::FirefoxSqlite)
        );
        assert_eq!(
            sniff(br#"{"roots":{"bookmark_bar":{}}}"#),
            Some(ImportFormat::ChromiumJson)
        );
        assert_eq!(
            sniff(b"<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<DL>"),
            Some(ImportFormat::NetscapeHtml)
        );
        assert_eq!(sniff(b"just some text"), None);
    }
}
