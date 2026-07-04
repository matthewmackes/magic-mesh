//! The **tree-sitter highlight engine** (EDITOR-5).
//!
//! Incremental per-language syntax highlighting over the rope
//! [`Buffer`](crate::buffer::Buffer), rendered through the shared Carbon
//! code-theme tokens ([`mde_egui::code::CodeToken`], §4).
//!
//! One [`Highlighter`] lives beside each open document whose file extension
//! maps to a vendored grammar ([`Language::from_path`]); an unknown extension
//! gets **no** highlighter — the honest plain-text render, never a guessed
//! grammar. The engine is pure state + methods over `(&mut Buffer, &Rope)`:
//! no egui, so every path below is unit-testable without a frame (§7).
//!
//! The per-frame protocol the widget drives:
//!
//! 1. [`Highlighter::sync`] — drain the buffer's pending
//!    [`EditDelta`](crate::buffer::EditDelta)s ([`Buffer::take_edits`]), feed
//!    each to `Tree::edit`, and re-parse **with the old tree** — tree-sitter's
//!    incremental path, O(edit) not O(file) per keystroke. The first sync (no
//!    tree yet) and a ledger overflow (`take_edits() == None`) do the one full
//!    parse. Parsing reads the rope chunk-by-chunk (`parse_with_options` over
//!    `chunk_at_byte`) — the document is never materialized as one string.
//! 2. [`Highlighter::spans_in`] — run the grammar's highlights query over just
//!    the **visible** char window (`QueryCursor::set_byte_range`, matching the
//!    widget's viewport culling) and fold the captures into non-overlapping
//!    [`HighlightSpan`]s. Later captures override earlier ones per char, which
//!    resolves nesting inner-wins (an escape inside a string arrives after the
//!    string in tree order), mirroring tree-sitter's own highlight semantics.
//!
//! Capture names are classified onto the small [`CodeToken`] vocabulary by
//! [`classify`]; a capture that maps to nothing (plain identifiers) simply
//! stays foreground text.

// The cast lints are allowed module-wide for the u32→usize capture-index and
// byte/char offset conversions — all bounded by the document size (the same
// rationale + repo precedent as `widget.rs`).
#![allow(clippy::cast_possible_truncation)]

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use mde_egui::code::CodeToken;
use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Language as Grammar, Node, Parser, Point, Query, QueryCursor, TextProvider, Tree,
};

use crate::buffer::{Buffer, EditDelta};

/// A language with a vendored tree-sitter grammar.
///
/// The set the design doc locks and the farm proved builds (rust, python, js,
/// ts, json, toml, markdown, bash). Selection is by file extension only
/// ([`Language::from_path`]); no shebang/content sniffing — predictable and
/// honest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// Rust (`.rs`).
    Rust,
    /// Python (`.py`, `.pyi`).
    Python,
    /// JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`).
    JavaScript,
    /// TypeScript (`.ts`, `.mts`, `.cts`, `.tsx`).
    TypeScript,
    /// JSON (`.json`).
    Json,
    /// TOML (`.toml`).
    Toml,
    /// Markdown (`.md`, `.markdown`) — the block grammar (headings, fences,
    /// lists); inline emphasis parsing is a later refinement.
    Markdown,
    /// Shell (`.sh`, `.bash`).
    Bash,
}

/// Every supported language, for iteration (tests + a future language picker).
pub const ALL_LANGUAGES: [Language; 8] = [
    Language::Rust,
    Language::Python,
    Language::JavaScript,
    Language::TypeScript,
    Language::Json,
    Language::Toml,
    Language::Markdown,
    Language::Bash,
];

impl Language {
    /// The language for `path`, by its extension (case-insensitive), or `None`
    /// for an unknown/absent extension — the caller then renders plain text.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// The language for a bare file extension (no dot), case-insensitive.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Self::TypeScript,
            "json" => Self::Json,
            "toml" => Self::Toml,
            "md" | "markdown" => Self::Markdown,
            "sh" | "bash" => Self::Bash,
            _ => return None,
        })
    }

    /// The human-readable name the chrome strip shows.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Markdown => "Markdown",
            Self::Bash => "Bash",
        }
    }

    /// The vendored grammar for this language.
    fn grammar(self) -> Grammar {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }

    /// The highlights-query source shipped with this language's grammar crate.
    /// TypeScript's query extends JavaScript's (upstream ships it that way), so
    /// the two are concatenated.
    fn query_source(self) -> String {
        match self {
            Self::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
            Self::Python => tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
            Self::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY.to_owned(),
            Self::TypeScript => format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            Self::Json => tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
            Self::Toml => tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
            Self::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
            Self::Bash => tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
        }
    }

    /// The compiled highlights [`Query`] for this language, built once per
    /// process and cached. `None` if the shipped query fails to compile against
    /// its own grammar (should never happen; degrades to plain text, no panic).
    fn query(self) -> Option<&'static Query> {
        static QUERIES: [OnceLock<Option<Query>>; ALL_LANGUAGES.len()] =
            [const { OnceLock::new() }; ALL_LANGUAGES.len()];
        QUERIES[self as usize]
            .get_or_init(|| Query::new(&self.grammar(), &self.query_source()).ok())
            .as_ref()
    }
}

/// Fold one query capture name onto the small [`CodeToken`] vocabulary, or
/// `None` for captures that should stay plain foreground text (identifiers,
/// parameters, embedded ranges). Matches on the leading dot-segments, so
/// `function.method` and `function.macro` both land on [`CodeToken::Function`]
/// while the more-specific `string.special.key` (a JSON object key) wins over
/// the bare `string` head.
fn classify(capture: &str) -> Option<CodeToken> {
    // Specific multi-segment names first.
    if capture.starts_with("string.special.key") {
        return Some(CodeToken::Property);
    }
    if capture.starts_with("string.escape") {
        return Some(CodeToken::Escape);
    }
    if capture.starts_with("variable.builtin") {
        // `self` / `this` — reads as a keyword-adjacent constant.
        return Some(CodeToken::Constant);
    }
    let head = capture.split('.').next().unwrap_or(capture);
    Some(match head {
        "comment" => CodeToken::Comment,
        "keyword" | "conditional" | "repeat" | "include" | "storageclass" | "exception" => {
            CodeToken::Keyword
        }
        "function" | "method" | "macro" => CodeToken::Function,
        "type" | "constructor" | "namespace" | "module" | "class" => CodeToken::Type,
        "string" | "character" => CodeToken::String,
        "escape" => CodeToken::Escape,
        "number" | "float" | "integer" => CodeToken::Number,
        "constant" | "boolean" => CodeToken::Constant,
        "property" | "field" | "key" | "parameter" => CodeToken::Property,
        "attribute" | "annotation" | "decorator" | "label" | "tag" => CodeToken::Attribute,
        "operator" => CodeToken::Operator,
        "punctuation" | "delimiter" => CodeToken::Punct,
        // Markdown-style markup captures ("text.title" / "markup.heading").
        "text" | "markup" => match capture.split('.').nth(1).unwrap_or("") {
            "title" | "heading" => CodeToken::Heading,
            "literal" | "raw" => CodeToken::String,
            "uri" | "reference" | "link" => CodeToken::Property,
            "quote" => CodeToken::Comment,
            _ => return None,
        },
        _ => return None,
    })
}

/// One contiguous run of same-token characters.
///
/// The widget paints each visible row's glyphs span by span in the span's
/// [`CodeToken`] color. `range` is in **char indices** into the rope (the
/// widget's native coordinate), sorted and non-overlapping within one
/// [`Highlighter::spans_in`] result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    /// The char-index range this token covers.
    pub range: Range<usize>,
    /// The semantic kind — painted via [`CodeToken::color`].
    pub token: CodeToken,
}

/// `TextProvider` over the rope for the query cursor's predicate evaluation
/// (`#eq?` / `#match?` need capture text): yields the node's bytes chunk by
/// chunk straight off the rope, never materializing the document.
struct RopeProvider<'a>(&'a Rope);

/// The chunk iterator [`RopeProvider`] hands the cursor.
struct ChunkBytes<'a>(ropey::iter::Chunks<'a>);

impl<'a> Iterator for ChunkBytes<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(str::as_bytes)
    }
}

impl<'a> TextProvider<&'a [u8]> for RopeProvider<'a> {
    type I = ChunkBytes<'a>;
    fn text(&mut self, node: Node) -> Self::I {
        let end = node.end_byte().min(self.0.len_bytes());
        let start = node.start_byte().min(end);
        ChunkBytes(self.0.byte_slice(start..end).chunks())
    }
}

/// The rope chunk containing byte `byte`, offset so the parser reads from
/// exactly that byte — the `parse_with_options` callback body.
fn chunk_at(rope: &Rope, byte: usize) -> &[u8] {
    if byte >= rope.len_bytes() {
        return &[];
    }
    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
    &chunk.as_bytes()[byte - chunk_start..]
}

/// An [`EditDelta`] as tree-sitter's `InputEdit`.
const fn input_edit(d: &EditDelta) -> InputEdit {
    InputEdit {
        start_byte: d.start_byte,
        old_end_byte: d.old_end_byte,
        new_end_byte: d.new_end_byte,
        start_position: point(d.start_point),
        old_end_position: point(d.old_end_point),
        new_end_position: point(d.new_end_point),
    }
}

/// A `(row, byte-column)` pair as a tree-sitter [`Point`].
const fn point((row, column): (usize, usize)) -> Point {
    Point { row, column }
}

/// The per-document highlight state: the language, its parser, and the live
/// syntax [`Tree`], kept in step with the buffer via [`sync`](Self::sync).
pub struct Highlighter {
    language: Language,
    parser: Parser,
    tree: Option<Tree>,
    full_parses: u32,
}

impl Highlighter {
    /// A highlighter for `language`, or `None` if the grammar/query fails to
    /// load (degrades to plain text — never a panic, §7-honest).
    #[must_use]
    pub fn new(language: Language) -> Option<Self> {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).ok()?;
        language.query()?; // no usable query → no highlighter
        Some(Self {
            language,
            parser,
            tree: None,
            full_parses: 0,
        })
    }

    /// A highlighter for the file at `path`, by extension — `None` for an
    /// unknown extension (the honest plain-text path).
    #[must_use]
    pub fn for_path(path: &Path) -> Option<Self> {
        Self::new(Language::from_path(path)?)
    }

    /// The language this highlighter parses.
    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    /// How many **full** (non-incremental) parses have run — instrumentation
    /// proving the per-keystroke path is the incremental one (it stays at 1
    /// across an edit + [`sync`](Self::sync) cycle; only the first parse and a
    /// ledger overflow bump it).
    #[must_use]
    pub const fn full_parses(&self) -> u32 {
        self.full_parses
    }

    /// Bring the syntax tree up to date with `buffer` — the once-per-frame call.
    ///
    /// Drains the buffer's pending [`EditDelta`]s; when a tree exists and the
    /// ledger did not overflow, each delta is spliced into it (`Tree::edit`) and
    /// the re-parse reuses the old tree (tree-sitter's incremental path). The
    /// first call, or an overflowed ledger, does the one full parse instead. A
    /// no-edit frame is a no-op.
    pub fn sync(&mut self, buffer: &mut Buffer) {
        let deltas = buffer.take_edits();
        if let (Some(tree), Some(deltas)) = (self.tree.as_mut(), deltas.as_deref()) {
            if deltas.is_empty() {
                return; // nothing changed — keep the tree as-is
            }
            // Splice each recorded edit into the old tree, then re-parse WITH
            // it — tree-sitter's incremental path (O(edit), not O(file)).
            for delta in deltas {
                tree.edit(&input_edit(delta));
            }
            self.reparse(buffer.rope(), true);
        } else {
            // First parse (no tree yet) or ledger overflow (`None`): the one
            // full parse.
            self.tree = None;
            self.reparse(buffer.rope(), false);
        }
    }

    /// Parse the rope, reusing the edited old tree when `incremental`. Reads
    /// chunk-by-chunk off the rope; on the (cancellation-only) `None` result the
    /// previous tree is kept rather than dropping highlights.
    fn reparse(&mut self, rope: &Rope, incremental: bool) {
        let old = if incremental {
            self.tree.as_ref()
        } else {
            None
        };
        let parsed = self
            .parser
            .parse_with_options(&mut |byte, _| chunk_at(rope, byte), old, None);
        if !incremental {
            self.full_parses = self.full_parses.saturating_add(1);
        }
        if let Some(tree) = parsed {
            self.tree = Some(tree);
        }
    }

    /// The foldable regions of the current syntax tree (EDITOR-12) — reuses the
    /// SAME parsed tree as highlighting, never a second parse. Empty until the
    /// first [`sync`](Self::sync) (or for a language whose tree failed to build).
    #[must_use]
    pub fn fold_regions(&self, _rope: &Rope) -> Vec<crate::fold::FoldRegion> {
        self.tree
            .as_ref()
            .map(crate::fold::regions_from_tree)
            .unwrap_or_default()
    }

    /// The document's outline symbols (functions / types / impls) from the current
    /// tree (EDITOR-12) — again over the SAME parsed tree. Names are read straight
    /// off `rope` by byte span. Empty until the first [`sync`](Self::sync).
    #[must_use]
    pub fn symbols(&self, rope: &Rope) -> Vec<crate::outline::Symbol> {
        self.tree
            .as_ref()
            .map(|tree| crate::outline::symbols_from_tree(tree, rope))
            .unwrap_or_default()
    }

    /// The highlight spans intersecting the char window `chars` — the widget
    /// calls this once per frame with the visible viewport (so the query cost
    /// scales with what's on screen, matching the paint culling), then slices
    /// the result per visible row.
    ///
    /// Returns sorted, non-overlapping spans clipped to the window; chars not
    /// covered by any span are plain foreground text. Empty until the first
    /// [`sync`](Self::sync).
    #[must_use]
    pub fn spans_in(&self, rope: &Rope, chars: Range<usize>) -> Vec<HighlightSpan> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let Some(query) = self.language.query() else {
            return Vec::new();
        };
        let len = rope.len_chars();
        let window = chars.start.min(len)..chars.end.min(len);
        if window.start >= window.end {
            return Vec::new();
        }
        let byte_lo = rope.char_to_byte(window.start);
        let byte_hi = rope.char_to_byte(window.end);

        // Per-char token slots over the window. Captures arrive in tree order,
        // so a later (inner / more specific) capture overwrites an earlier
        // outer one — the inner-wins flattening.
        let mut slots: Vec<Option<CodeToken>> = vec![None; window.end - window.start];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte_lo..byte_hi);
        let mut captures = cursor.captures(query, tree.root_node(), RopeProvider(rope));
        while let Some((matched, cap_idx)) = captures.next() {
            let capture = matched.captures[*cap_idx];
            let Some(name) = query.capture_names().get(capture.index as usize).copied() else {
                continue;
            };
            let Some(token) = classify(name) else {
                continue;
            };
            let start_byte = capture.node.start_byte().max(byte_lo);
            let end_byte = capture.node.end_byte().min(byte_hi);
            if start_byte >= end_byte {
                continue;
            }
            let start = rope.byte_to_char(start_byte) - window.start;
            let end = (rope.byte_to_char(end_byte) - window.start).min(slots.len());
            for slot in &mut slots[start..end] {
                *slot = Some(token);
            }
        }

        // Compress the slots into contiguous same-token runs.
        let mut spans = Vec::new();
        let mut i = 0;
        while i < slots.len() {
            let Some(token) = slots[i] else {
                i += 1;
                continue;
            };
            let run_start = i;
            while i < slots.len() && slots[i] == Some(token) {
                i += 1;
            }
            spans.push(HighlightSpan {
                range: window.start + run_start..window.start + i,
                token,
            });
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, HighlightSpan, Highlighter, Language, ALL_LANGUAGES};
    use crate::buffer::Buffer;
    use mde_egui::code::CodeToken;
    use std::path::Path;

    /// Parse `text` as `lang` and return the whole document's spans.
    fn highlight_all(lang: Language, text: &str) -> (Highlighter, Buffer, Vec<HighlightSpan>) {
        let mut buf = Buffer::from_text(text);
        let mut hl = Highlighter::new(lang).expect("grammar + query load");
        hl.sync(&mut buf);
        let spans = hl.spans_in(buf.rope(), 0..buf.len_chars());
        (hl, buf, spans)
    }

    /// The distinct token kinds present in `spans`.
    fn kinds(spans: &[HighlightSpan]) -> Vec<CodeToken> {
        let mut v: Vec<CodeToken> = Vec::new();
        for s in spans {
            if !v.contains(&s.token) {
                v.push(s.token);
            }
        }
        v
    }

    /// The text a span covers, for capture-placement asserts.
    fn span_text(buf: &Buffer, span: &HighlightSpan) -> String {
        buf.rope().slice(span.range.clone()).to_string()
    }

    // ── extension → language mapping ─────────────────────────────────────────

    #[test]
    fn extensions_map_to_their_languages() {
        assert_eq!(Language::from_extension("rs"), Some(Language::Rust));
        assert_eq!(Language::from_extension("py"), Some(Language::Python));
        assert_eq!(Language::from_extension("js"), Some(Language::JavaScript));
        assert_eq!(Language::from_extension("mjs"), Some(Language::JavaScript));
        assert_eq!(Language::from_extension("ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("tsx"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("json"), Some(Language::Json));
        assert_eq!(Language::from_extension("toml"), Some(Language::Toml));
        assert_eq!(Language::from_extension("md"), Some(Language::Markdown));
        assert_eq!(Language::from_extension("sh"), Some(Language::Bash));
        assert_eq!(
            Language::from_extension("RS"),
            Some(Language::Rust),
            "extension match is case-insensitive"
        );
        assert_eq!(Language::from_extension("xyz"), None);
        assert_eq!(Language::from_extension(""), None);
    }

    #[test]
    fn paths_route_through_their_extension() {
        assert_eq!(
            Language::from_path(Path::new("/src/main.rs")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_path(Path::new("Cargo.toml")),
            Some(Language::Toml)
        );
        assert_eq!(
            Language::from_path(Path::new("README")),
            None,
            "no extension"
        );
        assert_eq!(Language::from_path(Path::new("notes.txt")), None);
    }

    #[test]
    fn unknown_extension_yields_no_highlighter_and_stays_plain() {
        assert!(
            Highlighter::for_path(Path::new("notes.txt")).is_none(),
            "unknown extension → honest plain text, no guessed grammar"
        );
        assert!(Highlighter::for_path(Path::new("LICENSE")).is_none());
    }

    // ── per-language smoke: parse + expected capture kinds ──────────────────

    #[test]
    fn every_language_loads_its_grammar_and_query() {
        for lang in ALL_LANGUAGES {
            assert!(
                Highlighter::new(lang).is_some(),
                "{lang:?}: grammar or highlights query failed to load"
            );
        }
    }

    #[test]
    fn rust_snippet_yields_the_expected_kinds() {
        let (_, buf, spans) = highlight_all(
            Language::Rust,
            "// greet\nfn main() {\n    let s = \"hi\";\n    let n = 10;\n}\n",
        );
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Comment), "// greet: {kinds:?}");
        assert!(kinds.contains(&CodeToken::Keyword), "fn/let: {kinds:?}");
        assert!(kinds.contains(&CodeToken::Function), "main: {kinds:?}");
        assert!(kinds.contains(&CodeToken::String), "\"hi\": {kinds:?}");
        // The comment span sits exactly on the comment text.
        let comment = spans
            .iter()
            .find(|s| s.token == CodeToken::Comment)
            .expect("a comment span");
        assert_eq!(span_text(&buf, comment), "// greet");
    }

    #[test]
    fn python_snippet_yields_the_expected_kinds() {
        let (_, _, spans) =
            highlight_all(Language::Python, "# note\ndef f(x):\n    return \"s\"\n");
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Comment), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::Keyword), "def/return: {kinds:?}");
        assert!(kinds.contains(&CodeToken::Function), "f: {kinds:?}");
        assert!(kinds.contains(&CodeToken::String), "{kinds:?}");
    }

    #[test]
    fn javascript_snippet_yields_the_expected_kinds() {
        let (_, _, spans) =
            highlight_all(Language::JavaScript, "// c\nfunction f() { return 'x'; }\n");
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Comment), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::Keyword), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::Function), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::String), "{kinds:?}");
    }

    #[test]
    fn typescript_snippet_yields_the_expected_kinds() {
        let (_, _, spans) = highlight_all(
            Language::TypeScript,
            "const n: number = 1;\nfunction f(): string { return 'x'; }\n",
        );
        let kinds = kinds(&spans);
        assert!(
            kinds.contains(&CodeToken::Keyword),
            "const/function: {kinds:?}"
        );
        assert!(kinds.contains(&CodeToken::String), "{kinds:?}");
        assert!(
            !spans.is_empty(),
            "the combined js+ts query produced captures"
        );
    }

    #[test]
    fn json_snippet_yields_keys_and_literals() {
        let (_, _, spans) = highlight_all(Language::Json, "{\"key\": [1, true, \"v\"]}\n");
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Number), "1: {kinds:?}");
        assert!(kinds.contains(&CodeToken::Constant), "true: {kinds:?}");
        assert!(
            kinds.contains(&CodeToken::Property) || kinds.contains(&CodeToken::String),
            "the object key highlights: {kinds:?}"
        );
    }

    #[test]
    fn toml_snippet_yields_the_expected_kinds() {
        let (_, _, spans) = highlight_all(
            Language::Toml,
            "# c\n[server]\nport = 8080\nname = \"eagle\"\n",
        );
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Comment), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::Number), "8080: {kinds:?}");
        assert!(kinds.contains(&CodeToken::String), "\"eagle\": {kinds:?}");
    }

    #[test]
    fn markdown_heading_highlights() {
        let (_, _, spans) = highlight_all(
            Language::Markdown,
            "# Title\n\nbody text\n\n```\ncode\n```\n",
        );
        assert!(
            !spans.is_empty(),
            "the markdown block grammar captured the heading/fence"
        );
    }

    #[test]
    fn bash_snippet_yields_the_expected_kinds() {
        let (_, _, spans) =
            highlight_all(Language::Bash, "# c\nif true; then\n  echo \"hi\"\nfi\n");
        let kinds = kinds(&spans);
        assert!(kinds.contains(&CodeToken::Comment), "{kinds:?}");
        assert!(kinds.contains(&CodeToken::Keyword), "if/then/fi: {kinds:?}");
        assert!(kinds.contains(&CodeToken::String), "{kinds:?}");
    }

    // ── incremental re-parse ─────────────────────────────────────────────────

    #[test]
    fn an_edit_rehighlights_incrementally_not_via_full_reparse() {
        let mut buf = Buffer::from_text("fn main() {}\n");
        let mut hl = Highlighter::new(Language::Rust).expect("rust");
        hl.sync(&mut buf);
        assert_eq!(hl.full_parses(), 1, "the first sync is the one full parse");
        let before = hl.spans_in(buf.rope(), 0..buf.len_chars());
        assert!(
            !before.iter().any(|s| s.token == CodeToken::Comment),
            "no comment yet"
        );

        // Type a comment line at the end — through the buffer, so the edit
        // deltas flow to the highlighter exactly as the widget's typing does.
        let at = buf.len_chars();
        buf.insert(at, "// tail\n");
        hl.sync(&mut buf);

        assert_eq!(
            hl.full_parses(),
            1,
            "the edit re-parsed INCREMENTALLY (Tree::edit + old tree), not from scratch"
        );
        let after = hl.spans_in(buf.rope(), 0..buf.len_chars());
        let comment = after
            .iter()
            .find(|s| s.token == CodeToken::Comment)
            .expect("the new comment is captured after the incremental re-parse");
        assert_eq!(
            buf.rope().slice(comment.range.clone()).to_string(),
            "// tail",
            "the comment span sits exactly on the inserted text"
        );
    }

    #[test]
    fn an_edit_that_changes_meaning_updates_captures() {
        // `let x = 1;` — then wrap the 1 in quotes so the number becomes a string.
        let mut buf = Buffer::from_text("fn f() { let x = 1; }\n");
        let mut hl = Highlighter::new(Language::Rust).expect("rust");
        hl.sync(&mut buf);
        let before = hl.spans_in(buf.rope(), 0..buf.len_chars());
        assert!(
            before
                .iter()
                .any(|s| matches!(s.token, CodeToken::Number | CodeToken::Constant)),
            "1 highlights as a numeric literal: {before:?}"
        );

        buf.insert(17, "\""); // fn f() { let x = "1; }
        buf.insert(19, "\""); // fn f() { let x = "1"; }
        hl.sync(&mut buf);

        let after = hl.spans_in(buf.rope(), 0..buf.len_chars());
        let string = after
            .iter()
            .find(|s| s.token == CodeToken::String)
            .expect("the quoted literal now captures as a string");
        assert_eq!(buf.rope().slice(string.range.clone()).to_string(), "\"1\"");
        assert_eq!(hl.full_parses(), 1, "still zero extra full parses");
    }

    #[test]
    fn undo_rehighlights_back_incrementally() {
        let mut buf = Buffer::from_text("fn main() {}\n");
        let mut hl = Highlighter::new(Language::Rust).expect("rust");
        hl.sync(&mut buf);
        let at = buf.len_chars();
        buf.insert(at, "// gone\n");
        hl.sync(&mut buf);
        assert!(hl
            .spans_in(buf.rope(), 0..buf.len_chars())
            .iter()
            .any(|s| s.token == CodeToken::Comment));

        buf.undo();
        hl.sync(&mut buf);
        assert!(
            !hl.spans_in(buf.rope(), 0..buf.len_chars())
                .iter()
                .any(|s| s.token == CodeToken::Comment),
            "the undone comment's capture is gone"
        );
        assert_eq!(hl.full_parses(), 1, "undo also rode the incremental path");
    }

    #[test]
    fn a_ledger_overflow_falls_back_to_one_full_reparse() {
        let mut buf = Buffer::from_text("fn main() {}\n");
        let mut hl = Highlighter::new(Language::Rust).expect("rust");
        hl.sync(&mut buf);
        assert_eq!(hl.full_parses(), 1);

        // Blow past the ledger cap without an intervening sync.
        let at = buf.len_chars();
        for i in 0..1_500 {
            buf.insert(at + i, "a");
        }
        hl.sync(&mut buf);
        assert_eq!(
            hl.full_parses(),
            2,
            "an overflowed ledger triggers exactly one full reparse"
        );
        // And the tree is still live + correct.
        assert!(hl
            .spans_in(buf.rope(), 0..buf.len_chars())
            .iter()
            .any(|s| s.token == CodeToken::Keyword));
    }

    // ── window clipping + classification ─────────────────────────────────────

    #[test]
    fn spans_clip_to_the_requested_window() {
        let (hl, buf, _) = highlight_all(Language::Rust, "// aaaa\nfn main() {}\n");
        // A window over just the second line must not leak the comment span.
        let start = buf.line_to_char(1);
        let spans = hl.spans_in(buf.rope(), start..buf.len_chars());
        assert!(!spans.is_empty());
        for s in &spans {
            assert!(s.range.start >= start, "span leaked before the window");
            assert!(s.range.end <= buf.len_chars());
        }
        assert!(!spans.iter().any(|s| s.token == CodeToken::Comment));
        // An out-of-range / empty window is a clean empty result.
        assert!(hl.spans_in(buf.rope(), 500..900).is_empty());
        assert!(hl.spans_in(buf.rope(), 3..3).is_empty());
    }

    #[test]
    fn spans_are_sorted_and_non_overlapping() {
        let (_, _, spans) = highlight_all(
            Language::Rust,
            "fn main() { let s = \"a\\nb\"; }\n// tail\n",
        );
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "overlap/misorder: {pair:?}"
            );
        }
    }

    #[test]
    fn classification_folds_capture_names_onto_the_vocabulary() {
        assert_eq!(classify("keyword"), Some(CodeToken::Keyword));
        assert_eq!(classify("function.method"), Some(CodeToken::Function));
        assert_eq!(classify("function.macro"), Some(CodeToken::Function));
        assert_eq!(classify("type.builtin"), Some(CodeToken::Type));
        assert_eq!(classify("string.special.key"), Some(CodeToken::Property));
        assert_eq!(classify("constant.builtin"), Some(CodeToken::Constant));
        assert_eq!(classify("punctuation.bracket"), Some(CodeToken::Punct));
        assert_eq!(classify("text.title"), Some(CodeToken::Heading));
        assert_eq!(classify("variable"), None, "plain identifiers stay plain");
        assert_eq!(classify("embedded"), None);
    }
}
