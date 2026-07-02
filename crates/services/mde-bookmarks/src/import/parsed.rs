//! The browser-independent parsed tree the format importers produce.
//!
//! Every importer ([`super::firefox`], [`super::chromium`], [`super::netscape`])
//! parses its file into this one shape; the CRDT glue ([`super::plan`]) then
//! folds it into the model's ops (§6 glue-not-reimplementation — the parsers
//! never touch the CRDT). Keeping the parse output separate from the ops makes
//! each importer a pure `bytes -> ParsedTree` function that a fixture test can
//! assert on directly.

use crate::Source;

/// A parsed bookmark leaf — the fields an importer can carry into the model.
///
/// `favicon` is intentionally absent: importers do not carry icon bytes into
/// the op stream (favicons are content-addressed + lazily synced, the worker's
/// job — lock Q6), so a freshly-imported bookmark starts with no `favicon_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBookmark {
    /// The target URL, exactly as stored by the source browser (normalization is
    /// applied only to the dedup *key*, never to the stored URL).
    pub url: String,
    /// The display title.
    pub title: String,
    /// Tags carried from the source (Firefox tag folders → here; lock Q11).
    pub tags: Vec<String>,
    /// Wall time (ms since the Unix epoch) the bookmark was added, or `0` if the
    /// source did not record one.
    pub added_ms: u64,
}

/// A node in a parsed import tree: a named folder (with children) or a bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedNode {
    /// An interior folder.
    Folder {
        /// The folder display name.
        name: String,
        /// The folder's ordered children.
        children: Vec<Self>,
    },
    /// A leaf bookmark.
    Bookmark(ParsedBookmark),
}

/// A whole parsed import: which browser it came from plus its top-level nodes.
///
/// [`ParsedTree::source`] drives both the `Imported/<Browser>` subfolder name
/// and the [`Source`] stamped on every imported bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTree {
    /// The browser the tree was parsed from.
    pub source: Source,
    /// The top-level nodes (they land directly under `Imported/<Browser>`).
    pub roots: Vec<ParsedNode>,
}
