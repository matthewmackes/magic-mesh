//! AIR-11 (v6.1) — navigation stack + breadcrumb.
//!
//! `mde-music` navigates a stack of pages rooted at the hub. The
//! breadcrumb renders the path `Library → <segment> → …` capped at 4
//! visible segments (the middle is elided when deeper). This module is
//! the pure navigation model; the Iced view renders the breadcrumb +
//! routes clicks back through [`NavState`].

use crate::hub::HubCard;

/// The root breadcrumb label.
pub const ROOT_LABEL: &str = "Library";

/// Maximum breadcrumb segments shown before the middle is elided.
pub const MAX_SEGMENTS: usize = 4;

/// A navigable page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The 7-card hub (always the stack root).
    Hub,
    /// A hub category landing (Albums grid, Artists grid, …).
    Category(HubCard),
    /// A specific artist page (id, display name).
    Artist(String, String),
    /// A specific album page (id, display name).
    Album(String, String),
    /// A genre page — the albums in one genre (the genre name).
    Genre(String),
    /// A podcast channel page — its episodes (channel id, title).
    Podcast(String, String),
    /// MUSIC-RFX-6b — a playlist detail/reorder editor (playlist id, name).
    Playlist(String, String),
    /// A search-results page for a query.
    Search(String),
}

impl Route {
    /// Breadcrumb segment label for this route.
    #[must_use]
    pub fn segment(&self) -> String {
        match self {
            Self::Hub => ROOT_LABEL.to_string(),
            Self::Category(c) => c.label().to_string(),
            Self::Artist(_, name) | Self::Album(_, name) => name.clone(),
            Self::Genre(g) => g.clone(),
            Self::Podcast(_, name) | Self::Playlist(_, name) => name.clone(),
            Self::Search(q) => format!("Search: {q}"),
        }
    }
}

/// The navigation stack — never empty (always rooted at [`Route::Hub`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavState {
    stack: Vec<Route>,
}

impl Default for NavState {
    fn default() -> Self {
        Self {
            stack: vec![Route::Hub],
        }
    }
}

impl NavState {
    /// A fresh stack at the hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current (top) route.
    #[must_use]
    pub fn current(&self) -> &Route {
        self.stack.last().expect("nav stack is never empty")
    }

    /// Push a new page. Pushing the hub resets to root (a breadcrumb-root
    /// click); otherwise it extends the path.
    pub fn push(&mut self, route: Route) {
        if route == Route::Hub {
            self.reset();
        } else {
            self.stack.push(route);
        }
    }

    /// Pop one level (no-op at the root).
    pub fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// Reset to the hub root.
    pub fn reset(&mut self) {
        self.stack.truncate(1);
    }

    /// Ascend to the `index`-th breadcrumb segment (0 = root), dropping
    /// everything below it. Out-of-range indices are clamped.
    pub fn ascend_to(&mut self, index: usize) {
        let keep = (index + 1).min(self.stack.len()).max(1);
        self.stack.truncate(keep);
    }

    /// Depth (1 at the hub).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Breadcrumb segments to render. When the stack is deeper than
    /// [`MAX_SEGMENTS`], the middle collapses to an `…` ellipsis so the
    /// root + the last segments stay visible (Q3 max-4 lock).
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<String> {
        let labels: Vec<String> = self.stack.iter().map(Route::segment).collect();
        if labels.len() <= MAX_SEGMENTS {
            return labels;
        }
        // Keep the root + the last (MAX_SEGMENTS - 2) segments, with an
        // ellipsis between.
        let tail_count = MAX_SEGMENTS - 2;
        let mut out = vec![labels[0].clone(), "…".to_string()];
        out.extend(labels[labels.len() - tail_count..].iter().cloned());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_hub() {
        let n = NavState::new();
        assert_eq!(n.current(), &Route::Hub);
        assert_eq!(n.depth(), 1);
        assert_eq!(n.breadcrumb(), vec!["Library".to_string()]);
    }

    #[test]
    fn push_extends_and_pop_ascends() {
        let mut n = NavState::new();
        n.push(Route::Category(HubCard::Artists));
        n.push(Route::Artist("2".into(), "Air".into()));
        n.push(Route::Album("a1".into(), "Moon Safari".into()));
        assert_eq!(
            n.breadcrumb(),
            vec!["Library", "Artists", "Air", "Moon Safari"]
        );
        n.pop();
        assert_eq!(n.breadcrumb(), vec!["Library", "Artists", "Air"]);
    }

    #[test]
    fn pushing_hub_resets_to_root() {
        let mut n = NavState::new();
        n.push(Route::Category(HubCard::Albums));
        n.push(Route::Album("x".into(), "X".into()));
        n.push(Route::Hub); // breadcrumb-root click
        assert_eq!(n.depth(), 1);
        assert_eq!(n.current(), &Route::Hub);
    }

    #[test]
    fn ascend_to_drops_below() {
        let mut n = NavState::new();
        n.push(Route::Category(HubCard::Artists));
        n.push(Route::Artist("2".into(), "Air".into()));
        n.push(Route::Album("a1".into(), "Moon Safari".into()));
        n.ascend_to(1); // back to Artists
        assert_eq!(n.breadcrumb(), vec!["Library", "Artists"]);
        // Clamp: ascend past the end is a no-op-ish (clamped).
        n.ascend_to(99);
        assert_eq!(n.depth(), 2);
        // Ascend to root.
        n.ascend_to(0);
        assert_eq!(n.depth(), 1);
    }

    #[test]
    fn pop_at_root_is_noop() {
        let mut n = NavState::new();
        n.pop();
        assert_eq!(n.depth(), 1);
    }

    #[test]
    fn breadcrumb_elides_middle_when_deep() {
        let mut n = NavState::new();
        n.push(Route::Category(HubCard::Genres));
        n.push(Route::Artist("1".into(), "Jazz".into()));
        n.push(Route::Artist("2".into(), "Miles Davis".into()));
        n.push(Route::Album("3".into(), "Kind of Blue".into()));
        // depth 5 > 4 → Library, …, last 2.
        assert_eq!(
            n.breadcrumb(),
            vec!["Library", "…", "Miles Davis", "Kind of Blue"]
        );
    }

    #[test]
    fn search_route_segment() {
        assert_eq!(Route::Search("miles".into()).segment(), "Search: miles");
    }
}
