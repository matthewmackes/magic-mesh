//! Shared search/omnibox ranking model.
//!
//! This is the render-agnostic core of the `SEARCH-omnibox` epic: a pure scorer
//! that can rank app surfaces, files, mesh units, Browser visits/bookmarks, web
//! suggestions, and future assistant-ranked rows without binding that logic to
//! any one widget. Existing surfaces can adopt it incrementally while keeping
//! their own action dispatch paths.

/// The health of the node/source that produced a search candidate.
///
/// This is an **explicit ranking weight**, not a searchable text term: within a
/// single lexical tier (and fuzzy-cost bracket), a candidate from a healthier
/// source ranks above one from a degraded source, which ranks above one from a
/// down source — deterministically. Mesh rows carry the discovered unit's health
/// here; every other domain (and any unprobed source) is [`SourceHealth::Unknown`],
/// treated as a neutral baseline so health weighting only ever *penalises* an
/// unhealthy mesh source and never reorders unrelated local rows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceHealth {
    /// The source reports no active alarms.
    Healthy,
    /// A warning-tier alarm is active on the source.
    Degraded,
    /// The source is critical or unreachable.
    Down,
    /// Health is unprobed or the candidate is not a mesh source — neutral.
    #[default]
    Unknown,
}

impl SourceHealth {
    /// The ranking penalty for this tier; **lower sorts first**, so a healthy (or
    /// unknown/neutral) source leads, then degraded, then down. Healthy and
    /// Unknown share the neutral `0` so an unprobed or non-mesh row is never
    /// demoted below a genuinely-healthy one.
    const fn penalty(self) -> u8 {
        match self {
            Self::Healthy | Self::Unknown => 0,
            Self::Degraded => 1,
            Self::Down => 2,
        }
    }
}

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
    source_health: SourceHealth,
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
            source_health: SourceHealth::Unknown,
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

    /// Set the health of the source node that produced this candidate. Mesh rows
    /// use it so a healthy node ranks above a degraded one above a down one for an
    /// equal text match; other domains leave it [`SourceHealth::Unknown`] (neutral).
    /// It is an explicit ranking weight, applied *after* lexical exactness and
    /// *before* the softer `model_score` tie-break — a degraded source can never
    /// bury a stronger typed match.
    #[must_use]
    pub const fn with_source_health(mut self, health: SourceHealth) -> Self {
        self.source_health = health;
        self
    }

    /// The health weight of this candidate's source node. Callers that re-wrap a
    /// candidate (e.g. the shell folding Explorer's mesh rows into the front door)
    /// use this to carry the weight across the copy.
    #[must_use]
    pub const fn source_health(&self) -> SourceHealth {
        self.source_health
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
/// substring, auxiliary field substring, then fuzzy title. Within one lexical
/// tier the order is then, in decreasing strength: source health (healthy/unknown
/// over degraded over down — see [`SourceHealth`]), then model score, then stable
/// source order. Neither health nor model score is a license to bury a stronger
/// typed match: both only break ties inside the same tier.
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

    let mut scored: Vec<(MatchTier, (usize, usize), u8, i32, usize, SearchItem<T>)> = items
        .into_iter()
        .filter_map(|item| {
            let (tier, cost) = score_item(&q, &item)?;
            Some((
                tier,
                cost,
                item.source_health.penalty(),
                item.model_score.unwrap_or(0),
                item.source_rank,
                item,
            ))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(b.3.cmp(&a.3))
            .then(a.4.cmp(&b.4))
    });
    scored
        .into_iter()
        .take(cap)
        .map(|(tier, _, _, _, _, item)| SearchHit { item, tier })
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
    fn mesh_health_weight_ranks_healthy_over_degraded_over_down_for_equal_match() {
        // Same title, same lexical tier, same source order — only source health
        // differs, and it decides the order: healthy first, then degraded, then down.
        let items = vec![
            item(SearchDomain::Mesh, "oak-node", "peer:oak-down", "down")
                .with_source_health(SourceHealth::Down)
                .with_source_rank(0),
            item(
                SearchDomain::Mesh,
                "oak-node",
                "peer:oak-healthy",
                "healthy",
            )
            .with_source_health(SourceHealth::Healthy)
            .with_source_rank(1),
            item(
                SearchDomain::Mesh,
                "oak-node",
                "peer:oak-degraded",
                "degraded",
            )
            .with_source_health(SourceHealth::Degraded)
            .with_source_rank(2),
        ];

        let ids: Vec<&str> = ranked_hits("oak", items, 8)
            .into_iter()
            .map(|hit| hit.item.payload)
            .collect();

        assert_eq!(ids, ["healthy", "degraded", "down"]);
    }

    #[test]
    fn source_health_never_overrides_a_stronger_lexical_match() {
        // A down source with a title *prefix* match still beats a healthy source
        // that only matches lexically weaker — health is a within-tier tie-break,
        // never a license to bury a stronger typed match.
        let items = vec![
            item(SearchDomain::Mesh, "oak-router", "peer:oak", "down-prefix")
                .with_source_health(SourceHealth::Down),
            item(
                SearchDomain::Mesh,
                "core oak relay",
                "peer:core",
                "healthy-substr",
            )
            .with_source_health(SourceHealth::Healthy),
        ];

        let ids: Vec<&str> = ranked_hits("oak", items, 8)
            .into_iter()
            .map(|hit| hit.item.payload)
            .collect();

        assert_eq!(ids, ["down-prefix", "healthy-substr"]);
    }

    #[test]
    fn unknown_source_health_is_neutral_and_leaves_existing_order_intact() {
        // Default (Unknown) health shares the neutral weight of Healthy, so a
        // healthy row and an unprobed/non-mesh row tie on health and fall through
        // to stable source order — no collateral reordering of local rows.
        let items = vec![
            item(
                SearchDomain::App,
                "oak console",
                "surface:oak",
                "app-unknown",
            )
            .with_source_rank(0),
            item(
                SearchDomain::Mesh,
                "oak console",
                "peer:oak",
                "mesh-healthy",
            )
            .with_source_health(SourceHealth::Healthy)
            .with_source_rank(1),
        ];

        let ids: Vec<&str> = ranked_hits("oak", items, 8)
            .into_iter()
            .map(|hit| hit.item.payload)
            .collect();

        assert_eq!(ids, ["app-unknown", "mesh-healthy"]);
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
