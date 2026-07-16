//! Shared search/omnibox ranking model.
//!
//! This is the render-agnostic core of the `SEARCH-omnibox` epic: a pure scorer
//! that can rank app surfaces, files, mesh units, Browser visits/bookmarks, web
//! suggestions, and future assistant-ranked rows without binding that logic to
//! any one widget. Existing surfaces can adopt it incrementally while keeping
//! their own action dispatch paths.

/// The origin bucket of one unified omnibox candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchDomain {
    /// A shell application or surface.
    App,
    /// A file or directory from the Files model or a real file indexer.
    File,
    /// A discovered mesh unit, peer, service, or host.
    Mesh,
    /// A Browser bookmark.
    BrowserBookmark,
    /// A Browser history visit.
    BrowserHistory,
    /// A web-search suggestion from the configured search backend.
    WebSuggestion,
    /// A future assistant/model-ranked follow-up row.
    Assistant,
}

/// One candidate the unified omnibox can rank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchItem<T> {
    /// Source bucket for grouping and result decoration.
    pub domain: SearchDomain,
    /// Primary display text and first lexical match field.
    pub title: String,
    /// Secondary target text, usually a path, URL, surface route, or mesh URI.
    pub target: String,
    /// Additional searchable terms that should not replace the visible title.
    pub terms: Vec<String>,
    /// Caller-owned activation payload.
    pub payload: T,
    source_rank: usize,
    model_score: Option<i32>,
}

impl<T> SearchItem<T> {
    /// Build a candidate with no auxiliary terms and default source order.
    pub fn new(
        domain: SearchDomain,
        title: impl Into<String>,
        target: impl Into<String>,
        payload: T,
    ) -> Self {
        Self {
            domain,
            title: title.into(),
            target: target.into(),
            terms: Vec::new(),
            payload,
            source_rank: 0,
            model_score: None,
        }
    }

    /// Add auxiliary searchable terms such as tags, groups, type labels, or peer names.
    #[must_use]
    pub fn with_terms(mut self, terms: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.terms = terms.into_iter().map(Into::into).collect();
        self
    }

    /// Set stable source order for tie-breaking inside the same match tier.
    #[must_use]
    pub const fn with_source_rank(mut self, rank: usize) -> Self {
        self.source_rank = rank;
        self
    }

    /// Optional rank supplied by a later local/AI scorer. It only breaks ties
    /// inside the same lexical tier; exact typed matches still lead.
    #[must_use]
    pub const fn with_model_score(mut self, score: i32) -> Self {
        self.model_score = Some(score);
        self
    }
}

/// A ranked hit with enough metadata for callers to render sections/debug tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit<T> {
    /// The matched candidate.
    pub item: SearchItem<T>,
    /// The lexical tier that made this candidate match.
    pub tier: MatchTier,
}

/// Lexical match class, ordered from strongest to weakest.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MatchTier {
    /// Query matches the beginning of the title.
    TitlePrefix,
    /// Query matches the beginning of the target, including URL host starts.
    TargetPrefix,
    /// Query appears inside the title.
    TitleSubstring,
    /// Query appears in the target or auxiliary terms.
    AuxiliarySubstring,
    /// Query is a subsequence of the title.
    FuzzyTitle,
}

/// Rank `items` for `query`, best first, capped at `cap`.
///
/// Lexical exactness always wins: title prefix, target/URL prefix, title
/// substring, auxiliary field substring, then fuzzy title. Model score is a
/// tie-breaker within one tier, not a license to bury a direct typed match.
#[must_use]
pub fn ranked_hits<T: Clone>(
    query: &str,
    items: impl IntoIterator<Item = SearchItem<T>>,
    cap: usize,
) -> Vec<SearchHit<T>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || cap == 0 {
        return Vec::new();
    }

    let mut scored: Vec<(MatchTier, (usize, usize), i32, usize, SearchItem<T>)> = items
        .into_iter()
        .filter_map(|item| {
            let (tier, cost) = score_item(&q, &item)?;
            Some((
                tier,
                cost,
                item.model_score.unwrap_or(0),
                item.source_rank,
                item,
            ))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    scored
        .into_iter()
        .take(cap)
        .map(|(tier, _, _, _, item)| SearchHit { item, tier })
        .collect()
}

fn score_item<T>(query: &str, item: &SearchItem<T>) -> Option<(MatchTier, (usize, usize))> {
    let title = item.title.to_lowercase();
    let target = item.target.to_lowercase();
    if title.starts_with(query) {
        return Some((MatchTier::TitlePrefix, (0, 0)));
    }
    if target_prefix_matches(&target, query) {
        return Some((MatchTier::TargetPrefix, (0, 0)));
    }
    if title.contains(query) {
        return Some((MatchTier::TitleSubstring, (0, 0)));
    }
    if target.contains(query)
        || item
            .terms
            .iter()
            .any(|term| term.to_lowercase().contains(query))
    {
        return Some((MatchTier::AuxiliarySubstring, (0, 0)));
    }
    fuzzy_cost(&title, query).map(|cost| (MatchTier::FuzzyTitle, cost))
}

fn target_prefix_matches(target: &str, query: &str) -> bool {
    target.starts_with(query) || target.contains(&format!("://{query}"))
}

/// Fuzzy subsequence score of `query` against lowercased `title`.
///
/// Lower is better: fewer gaps, then an earlier first matched char.
fn fuzzy_cost(title: &str, query: &str) -> Option<(usize, usize)> {
    let mut haystack = title.char_indices();
    let mut first: Option<usize> = None;
    let mut last = 0;
    for needle in query.chars() {
        let (at, _) = haystack.by_ref().find(|&(_, c)| c == needle)?;
        first.get_or_insert(at);
        last = at;
    }
    first.map(|start| (last - start, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        domain: SearchDomain,
        title: &str,
        target: &str,
        id: &'static str,
    ) -> SearchItem<&'static str> {
        SearchItem::new(domain, title, target, id)
    }

    #[test]
    fn empty_queries_and_zero_caps_return_no_hits() {
        let items = [item(
            SearchDomain::App,
            "Browser",
            "surface:browser",
            "browser",
        )];

        assert!(ranked_hits("  ", items.clone(), 8).is_empty());
        assert!(ranked_hits("browser", items, 0).is_empty());
    }

    #[test]
    fn unified_omnibox_accepts_apps_files_mesh_browser_web_and_assistant_sources() {
        let items = vec![
            item(SearchDomain::App, "Browser", "surface:browser", "app"),
            item(
                SearchDomain::File,
                "browser-notes.md",
                "/home/mde/browser-notes.md",
                "file",
            ),
            item(
                SearchDomain::Mesh,
                "build-browser-node",
                "mesh://build-browser-node",
                "mesh",
            ),
            item(
                SearchDomain::BrowserBookmark,
                "Browser Backlog",
                "https://docs.mesh/browser",
                "bookmark",
            ),
            item(
                SearchDomain::BrowserHistory,
                "Daily Browser Run",
                "https://history.mesh/browser",
                "history",
            ),
            item(
                SearchDomain::WebSuggestion,
                "browser engine status",
                "https://search.mesh/search?q=browser+engine+status",
                "web",
            ),
            item(
                SearchDomain::Assistant,
                "Browser debugging summary",
                "assistant://search/browser",
                "assistant",
            ),
        ];

        let domains: Vec<SearchDomain> = ranked_hits("browser", items, 16)
            .into_iter()
            .map(|hit| hit.item.domain)
            .collect();

        assert!(domains.contains(&SearchDomain::App));
        assert!(domains.contains(&SearchDomain::File));
        assert!(domains.contains(&SearchDomain::Mesh));
        assert!(domains.contains(&SearchDomain::BrowserBookmark));
        assert!(domains.contains(&SearchDomain::BrowserHistory));
        assert!(domains.contains(&SearchDomain::WebSuggestion));
        assert!(domains.contains(&SearchDomain::Assistant));
    }

    #[test]
    fn lexical_tiers_match_browser_and_start_menu_expectations() {
        let items = vec![
            item(
                SearchDomain::BrowserBookmark,
                "Rust Book",
                "https://doc.rust-lang.org/book/",
                "title-prefix",
            ),
            item(
                SearchDomain::BrowserHistory,
                "Language Home",
                "https://rust-lang.org/",
                "url-prefix",
            ),
            item(
                SearchDomain::App,
                "Trust Center",
                "surface:trust",
                "title-substring",
            ),
            item(
                SearchDomain::App,
                "Workbench",
                "surface:workbench",
                "aux-substring",
            )
            .with_terms(["Rust Ops"]),
            item(SearchDomain::App, "Run Stats", "surface:run-stats", "fuzzy"),
        ];

        let hits = ranked_hits("rust", items, 8);
        let ids: Vec<&str> = hits.iter().map(|hit| hit.item.payload).collect();
        let tiers: Vec<MatchTier> = hits.iter().map(|hit| hit.tier).collect();

        assert_eq!(
            ids,
            [
                "title-prefix",
                "url-prefix",
                "title-substring",
                "aux-substring",
                "fuzzy"
            ]
        );
        assert_eq!(
            tiers,
            [
                MatchTier::TitlePrefix,
                MatchTier::TargetPrefix,
                MatchTier::TitleSubstring,
                MatchTier::AuxiliarySubstring,
                MatchTier::FuzzyTitle,
            ]
        );
    }

    #[test]
    fn model_score_breaks_ties_without_overriding_typed_match_quality() {
        let items = vec![
            item(
                SearchDomain::File,
                "mesh report",
                "/docs/a.md",
                "low-prefix",
            )
            .with_model_score(1)
            .with_source_rank(0),
            item(
                SearchDomain::Mesh,
                "mesh router",
                "mesh://router",
                "high-prefix",
            )
            .with_model_score(90)
            .with_source_rank(1),
            item(
                SearchDomain::Assistant,
                "Router summary",
                "assistant://mesh",
                "high-aux",
            )
            .with_model_score(500)
            .with_source_rank(2),
        ];

        let ids: Vec<&str> = ranked_hits("mesh", items, 8)
            .into_iter()
            .map(|hit| hit.item.payload)
            .collect();

        assert_eq!(ids, ["high-prefix", "low-prefix", "high-aux"]);
    }

    #[test]
    fn fuzzy_tie_break_prefers_tighter_title_match_then_source_order() {
        let items = vec![
            item(SearchDomain::App, "Storage", "surface:storage", "storage").with_source_rank(0),
            item(SearchDomain::App, "Browser", "surface:browser", "browser").with_source_rank(1),
            item(
                SearchDomain::App,
                "Broadcaster",
                "surface:broadcast",
                "broadcaster",
            )
            .with_source_rank(2),
        ];

        let ids: Vec<&str> = ranked_hits("sr", items, 8)
            .into_iter()
            .map(|hit| hit.item.payload)
            .collect();

        assert_eq!(ids, ["browser", "storage", "broadcaster"]);
    }
}
