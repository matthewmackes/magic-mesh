//! Smart clipboard (TERM-9) — selection classifiers + surface routing.
//!
//! Two pure, headless cores the widget glues onto the existing grid + selection
//! plumbing (§6 — no new terminal, no second scrollback):
//!
//! * **smart selection** — given a clicked column on a grid row, [`smart_span`]
//!   grows the selection to the right *thing*: a URL, a filesystem path, or a
//!   word (double-click); [`line_span`] takes the whole line (triple-click).
//! * **surface routing** — [`detect_launch`] classifies the token under the
//!   pointer and, per design lock Q12, routes a URL to the **Bookmarks** mesh
//!   browser and a path to the **Files** surface. The route is dispatched over
//!   the mesh Bus through the injectable [`LaunchBus`] seam (mirrors
//!   [`crate::remote::PtyBus`]): production publishes a typed
//!   [`OPEN_TOPIC`] request; tests record it.
//!
//! Rows are handled as `&[char]` (one cell = one column), so the widget passes a
//! visible row's glyphs straight in and the spans map back to cell columns.

use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// What the pointer landed on — the four selection granularities of design lock
/// Q12 ("selection + smart URL/path detection") plus whole-line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SmartKind {
    /// A run of word characters (alphanumerics + `_`).
    Word,
    /// A `scheme://…` (or `www.…`) hyperlink.
    Url,
    /// A filesystem path (`/…`, `~/…`, `./…`, `../…`, or a bare `~`).
    Path,
    /// The whole visible line.
    Line,
}

/// Where a detected token opens — the routing half of the smart clipboard.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LaunchRoute {
    /// A URL → the Bookmarks mesh browser.
    Bookmarks(String),
    /// A filesystem path → the Files surface.
    Files(String),
}

/// The Bus topic a launch request is published on — the mesh "surface-launch
/// path" (§6): the shell's dock drains it to raise the target surface. MUST
/// match the consumer's subscription.
pub const OPEN_TOPIC: &str = "action/desktop/open";

/// The typed body published on [`OPEN_TOPIC`] — a local serde mirror of the
/// dock's open request, exactly as [`crate::remote`] mirrors the PTY broker's
/// wire shapes.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LaunchRequest {
    /// The target surface: `"bookmarks"` or `"files"`.
    pub surface: String,
    /// The URL or path to open there.
    pub target: String,
}

impl LaunchRequest {
    /// The request for a route.
    #[must_use]
    pub fn of(route: &LaunchRoute) -> Self {
        match route {
            LaunchRoute::Bookmarks(url) => Self {
                surface: "bookmarks".to_string(),
                target: url.clone(),
            },
            LaunchRoute::Files(path) => Self {
                surface: "files".to_string(),
                target: path.clone(),
            },
        }
    }
}

/// The URL classifier: a scheme (`https://`, `ssh://`, `mesh://`, …) or a bare
/// `www.` prefix, then any non-space run. Compiled once.
fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:[a-zA-Z][a-zA-Z0-9+.\-]*://|www\.)\S+$").expect("static URL regex")
    })
}

/// Trailing characters trimmed off a detected token — the punctuation that
/// commonly *follows* a URL/path in prose but isn't part of it.
const TRAILERS: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"', '>'];

/// Whether a char counts as part of a word (double-click without a URL/path).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The maximal non-whitespace token containing `col`, as `[start, end)`.
/// `None` when `col` is out of range or sits on whitespace.
fn token_span(row: &[char], col: usize) -> Option<(usize, usize)> {
    if col >= row.len() || row[col].is_whitespace() {
        return None;
    }
    let mut start = col;
    while start > 0 && !row[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col + 1;
    while end < row.len() && !row[end].is_whitespace() {
        end += 1;
    }
    Some((start, end))
}

/// The maximal word-character run containing `col`, as `[start, end)`.
fn word_span(row: &[char], col: usize) -> Option<(usize, usize)> {
    if col >= row.len() || !is_word_char(row[col]) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word_char(row[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < row.len() && is_word_char(row[end]) {
        end += 1;
    }
    Some((start, end))
}

/// Whether `token` looks like a filesystem path we auto-open. Deliberately
/// conservative — only unambiguous path shapes, so a stray word never
/// mis-launches the Files surface.
fn looks_like_path(token: &str) -> bool {
    token == "~"
        || token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
}

/// Classify a already-trimmed token.
#[must_use]
pub fn classify(token: &str) -> SmartKind {
    if url_re().is_match(token) {
        SmartKind::Url
    } else if looks_like_path(token) {
        SmartKind::Path
    } else {
        SmartKind::Word
    }
}

/// The smart-selection span under a double-click at `col`.
///
/// A URL or path token selects the whole (trailing-punctuation-trimmed) token;
/// anything else selects the word. Returns the span `[start, end)` and its
/// kind, or `None` on whitespace.
#[must_use]
pub fn smart_span(row: &[char], col: usize) -> Option<(SmartKind, usize, usize)> {
    let (ts, te) = token_span(row, col)?;
    let token: String = row[ts..te].iter().collect();
    let trimmed = token.trim_end_matches(TRAILERS);
    let te_trim = ts + trimmed.chars().count();
    match classify(trimmed) {
        // A URL/path token is one unbroken span — take it whole (trimmed), but
        // only while the click is still inside the trimmed part.
        kind @ (SmartKind::Url | SmartKind::Path) if col < te_trim => Some((kind, ts, te_trim)),
        // Otherwise fall back to the word boundary.
        _ => word_span(row, col).map(|(s, e)| (SmartKind::Word, s, e)),
    }
}

/// The whole-line content span (first to last non-blank cell), or `None` when
/// the row is blank — the triple-click selection.
#[must_use]
pub fn line_span(row: &[char]) -> Option<(usize, usize)> {
    let start = row.iter().position(|c| !c.is_whitespace())?;
    let end = row.iter().rposition(|c| !c.is_whitespace())? + 1;
    Some((start, end))
}

/// The launch route for a click at `col`, or `None` when the token is a plain
/// word (or whitespace). A URL routes to Bookmarks, a path to Files.
#[must_use]
pub fn detect_launch(row: &[char], col: usize) -> Option<LaunchRoute> {
    let (kind, start, end) = smart_span(row, col)?;
    let text: String = row[start..end].iter().collect();
    route(kind, &text)
}

/// Route a classified token to its surface. A plain word has no route.
#[must_use]
pub fn route(kind: SmartKind, text: &str) -> Option<LaunchRoute> {
    match kind {
        SmartKind::Url => Some(LaunchRoute::Bookmarks(text.to_string())),
        SmartKind::Path => Some(LaunchRoute::Files(text.to_string())),
        SmartKind::Word | SmartKind::Line => None,
    }
}

/// The Bus seam a launch is dispatched over — publish a typed open request for
/// the shell's dock to raise the target surface.
///
/// Injectable so the routing fold is unit-tested headless (a recorder) while
/// production talks the live Bus ([`BusLaunchClient`]).
pub trait LaunchBus: Send + Sync {
    /// Publish `route` on [`OPEN_TOPIC`].
    ///
    /// # Errors
    /// An operator-readable string when the append can't be written (e.g. no
    /// Bus dir on this node); it never blocks.
    fn open(&self, route: &LaunchRoute) -> Result<(), String>;
}

/// The live Bus-backed launcher — a synchronous local `Persist` append, the
/// same persist-first path the remote-terminal client uses. Degrades honestly
/// to an error when this node has no Bus dir.
#[derive(Debug, Clone)]
pub struct BusLaunchClient {
    bus_root: Option<PathBuf>,
}

impl BusLaunchClient {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir).
    #[must_use]
    pub const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl LaunchBus for BusLaunchClient {
    fn open(&self, route: &LaunchRoute) -> Result<(), String> {
        let Some(root) = self.bus_root.as_ref() else {
            return Err(
                "No mesh Bus directory — can't open the target surface on this node.".to_string(),
            );
        };
        let body = serde_json::to_string(&LaunchRequest::of(route))
            .map_err(|e| format!("Couldn't encode the surface-open request: {e}"))?;
        mde_bus::persist::Persist::open(root.clone())
            .and_then(|p| {
                p.write(
                    OPEN_TOPIC,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&body),
                )
            })
            .map(|_| ())
            .map_err(|e| format!("Couldn't publish the surface-open request: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn classify_separates_urls_paths_and_words() {
        assert_eq!(classify("https://mesh.local/x"), SmartKind::Url);
        assert_eq!(classify("ssh://oak:22"), SmartKind::Url);
        assert_eq!(classify("www.example.com"), SmartKind::Url);
        assert_eq!(classify("/etc/hosts"), SmartKind::Path);
        assert_eq!(classify("~/notes.md"), SmartKind::Path);
        assert_eq!(classify("./build.sh"), SmartKind::Path);
        assert_eq!(classify("../sibling"), SmartKind::Path);
        assert_eq!(classify("~"), SmartKind::Path);
        assert_eq!(classify("hello"), SmartKind::Word);
        // A relative token with a slash but no path anchor stays a word (no
        // false Files launch).
        assert_eq!(classify("src/main.rs"), SmartKind::Word);
    }

    #[test]
    fn smart_span_picks_the_url_token_whole() {
        let r = row("see https://mesh.local/docs now");
        // Click inside the URL (col 10) → the whole URL span, kind Url.
        let (kind, s, e) = smart_span(&r, 10).expect("span");
        assert_eq!(kind, SmartKind::Url);
        assert_eq!(
            &r[s..e].iter().collect::<String>(),
            "https://mesh.local/docs"
        );
    }

    #[test]
    fn smart_span_trims_trailing_punctuation_from_a_url() {
        let r = row("(see https://mesh.local/x).");
        // Click inside the URL; the trailing ")." is not part of it.
        let (kind, s, e) = smart_span(&r, 12).expect("span");
        assert_eq!(kind, SmartKind::Url);
        assert_eq!(&r[s..e].iter().collect::<String>(), "https://mesh.local/x");
    }

    #[test]
    fn smart_span_picks_a_path_then_a_word() {
        let r = row("edit ~/notes.md please");
        let (kind, s, e) = smart_span(&r, 6).expect("path span");
        assert_eq!(kind, SmartKind::Path);
        assert_eq!(&r[s..e].iter().collect::<String>(), "~/notes.md");
        // A plain word double-click selects the word only.
        let (kind, s, e) = smart_span(&r, 18).expect("word span");
        assert_eq!(kind, SmartKind::Word);
        assert_eq!(&r[s..e].iter().collect::<String>(), "please");
    }

    #[test]
    fn smart_span_on_whitespace_is_none() {
        let r = row("a  b");
        assert_eq!(smart_span(&r, 1), None);
    }

    #[test]
    fn line_span_trims_surrounding_blanks() {
        assert_eq!(line_span(&row("  hi there  ")), Some((2, 10)));
        assert_eq!(line_span(&row("     ")), None);
    }

    #[test]
    fn detect_launch_routes_urls_and_paths() {
        let r = row("open https://a.b/c or /etc/hosts");
        assert_eq!(
            detect_launch(&r, 8),
            Some(LaunchRoute::Bookmarks("https://a.b/c".to_string()))
        );
        assert_eq!(
            detect_launch(&r, 24),
            Some(LaunchRoute::Files("/etc/hosts".to_string()))
        );
        // A word doesn't launch anything.
        assert_eq!(detect_launch(&r, 0), None);
    }

    #[test]
    fn route_maps_kinds_to_surfaces() {
        assert_eq!(
            route(SmartKind::Url, "https://x"),
            Some(LaunchRoute::Bookmarks("https://x".to_string()))
        );
        assert_eq!(
            route(SmartKind::Path, "/x"),
            Some(LaunchRoute::Files("/x".to_string()))
        );
        assert_eq!(route(SmartKind::Word, "x"), None);
        assert_eq!(route(SmartKind::Line, "x"), None);
    }

    #[test]
    fn launch_request_encodes_the_surface_and_target() {
        let req = LaunchRequest::of(&LaunchRoute::Bookmarks("https://x".into()));
        assert_eq!(req.surface, "bookmarks");
        assert_eq!(req.target, "https://x");
        let req = LaunchRequest::of(&LaunchRoute::Files("/x".into()));
        assert_eq!(req.surface, "files");
    }

    /// A recording seam — the headless twin of the live Bus client.
    #[derive(Default)]
    struct Recorder {
        routes: std::sync::Mutex<Vec<LaunchRoute>>,
    }
    impl LaunchBus for Recorder {
        fn open(&self, route: &LaunchRoute) -> Result<(), String> {
            self.routes.lock().expect("lock").push(route.clone());
            Ok(())
        }
    }

    #[test]
    fn a_launch_bus_receives_the_routed_open() {
        let rec = Recorder::default();
        let r = row("go https://mesh/x");
        let route = detect_launch(&r, 5).expect("url route");
        rec.open(&route).expect("record");
        assert_eq!(rec.routes.lock().expect("lock").as_slice(), &[route]);
    }

    #[test]
    fn bus_launch_client_publishes_a_real_open_request() {
        // The live client round-trips through a real Bus spool (tempdir),
        // exactly the persist-first path the remote terminal uses.
        let dir = std::env::temp_dir().join(format!("mde-term-launch-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok(); // clear any stale spool from a prior run
        std::fs::create_dir_all(&dir).expect("mkdir");
        let client = BusLaunchClient::with_root(Some(dir.clone()));
        client
            .open(&LaunchRoute::Files("/etc/hosts".into()))
            .expect("publish");

        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open persist");
        let msgs = persist.list_since(OPEN_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "one open request landed on the topic");
        let body = msgs[0].body.as_deref().expect("body");
        let req: LaunchRequest = serde_json::from_str(body).expect("decode");
        assert_eq!(req.surface, "files");
        assert_eq!(req.target, "/etc/hosts");

        // No-Bus node degrades to an honest error, never a panic.
        assert!(BusLaunchClient::with_root(None)
            .open(&LaunchRoute::Bookmarks("https://x".into()))
            .is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
