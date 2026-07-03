//! Scrollback search (TERM-9) — a typed find over the engine's grid rows.
//!
//! [`Search`] is the pure, headless core: a query (literal or regex), a case
//! mode, and the list of [`Match`]es it finds by scanning a [`Screen`]
//! row-by-row (§6 — it reads the same immutable snapshot the renderer paints,
//! never its own copy of the scrollback). The widget owns one, drives it from
//! the keyboard (`Ctrl+Shift+F`), highlights the matches through `Style`
//! tokens, and scrolls the current match into view — but every find/next/prev/
//! wrap decision is decided here and unit-tested against synthetic grids.
//!
//! Matches are line-bounded (a match never spans a row), the standard terminal
//! find behaviour; a `Match::row` is an absolute snapshot row (row `history` is
//! the first live viewport line), so it lines up with the widget's selection
//! address space and can be scrolled to directly.

use regex::RegexBuilder;

use crate::screen::Screen;

/// How the query's letter case is matched.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CaseMode {
    /// Case-insensitive unless the query itself contains an uppercase letter
    /// (the "smart case" every modern finder uses) — the default.
    #[default]
    Smart,
    /// Always case-sensitive.
    Sensitive,
    /// Always case-insensitive.
    Insensitive,
}

impl CaseMode {
    /// The next mode in the `Smart → Sensitive → Insensitive` cycle (the toggle
    /// the widget wires to a key).
    #[must_use]
    pub const fn cycled(self) -> Self {
        match self {
            Self::Smart => Self::Sensitive,
            Self::Sensitive => Self::Insensitive,
            Self::Insensitive => Self::Smart,
        }
    }

    /// A one-word label for the search bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::Sensitive => "case",
            Self::Insensitive => "nocase",
        }
    }
}

/// One found span: `len` cells starting at `col` on absolute row `row`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Match {
    /// Absolute snapshot row (history rows first, then the live viewport).
    pub row: usize,
    /// Start column of the match.
    pub col: usize,
    /// Match width in cells.
    pub len: usize,
}

/// The scrollback-search state: a query + mode + the matches over the last
/// [`recompute`](Search::recompute)d screen, plus which match is "current".
#[derive(Clone, Debug, Default)]
pub struct Search {
    active: bool,
    query: String,
    regex: bool,
    case: CaseMode,
    matches: Vec<Match>,
    current: Option<usize>,
    /// An invalid-regex message (set → zero matches, an honest note, no panic).
    error: Option<String>,
    /// The query/mode changed since the last recompute — the widget rescans.
    dirty: bool,
}

impl Search {
    /// A fresh, closed search.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the search overlay is open (grabbing typed keys for the query).
    #[must_use]
    pub const fn active(&self) -> bool {
        self.active
    }

    /// Open the overlay (a rescan is scheduled so a resumed query re-finds).
    pub const fn open(&mut self) {
        self.active = true;
        self.dirty = true;
    }

    /// Close the overlay. The query is kept so reopening resumes it.
    pub const fn close(&mut self) {
        self.active = false;
    }

    /// Open when closed, close when open.
    pub const fn toggle(&mut self) {
        if self.active {
            self.close();
        } else {
            self.open();
        }
    }

    /// The current query text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the query is interpreted as a regular expression.
    #[must_use]
    pub const fn is_regex(&self) -> bool {
        self.regex
    }

    /// The case mode.
    #[must_use]
    pub const fn case(&self) -> CaseMode {
        self.case
    }

    /// An honest note when the query is an invalid regex (empty otherwise).
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Append typed text to the query (a rescan is scheduled).
    pub fn push_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.query.push_str(text);
        self.dirty = true;
    }

    /// Drop the last character of the query (a rescan is scheduled).
    pub fn pop_char(&mut self) {
        if self.query.pop().is_some() {
            self.dirty = true;
        }
    }

    /// Replace the whole query (a rescan is scheduled).
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.dirty = true;
    }

    /// Flip literal ⇄ regex interpretation (a rescan is scheduled).
    pub const fn toggle_regex(&mut self) {
        self.regex = !self.regex;
        self.dirty = true;
    }

    /// Cycle the case mode (a rescan is scheduled).
    pub const fn cycle_case(&mut self) {
        self.case = self.case.cycled();
        self.dirty = true;
    }

    /// Whether a rescan is pending (the widget rescans when this or the
    /// scrollback length has changed since the last [`recompute`](Search::recompute)).
    #[must_use]
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    /// Every match found in the last scan, in reading order (row then column).
    #[must_use]
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    /// How many matches the last scan found.
    #[must_use]
    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// The index of the current match (`1`-based display value is `+1`).
    #[must_use]
    pub const fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// The current match span, or `None` when there are none.
    #[must_use]
    pub fn current_match(&self) -> Option<Match> {
        self.current.and_then(|i| self.matches.get(i).copied())
    }

    /// The absolute row of the current match (what the widget scrolls to).
    #[must_use]
    pub fn current_row(&self) -> Option<usize> {
        self.current_match().map(|m| m.row)
    }

    /// Advance to the next match, wrapping past the last back to the first.
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            self.current = None;
            return;
        }
        let n = self.matches.len();
        self.current = Some(self.current.map_or(0, |c| (c + 1) % n));
    }

    /// Step to the previous match, wrapping past the first back to the last.
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            self.current = None;
            return;
        }
        let n = self.matches.len();
        self.current = Some(self.current.map_or(n - 1, |c| (c + n - 1) % n));
    }

    /// Rescan `screen` for the query, rebuilding the match list. Clears the
    /// dirty flag. An empty query finds nothing; an invalid regex records an
    /// error and finds nothing (never a panic). The current index is preserved
    /// where it can be, else it lands on the first match.
    pub fn recompute(&mut self, screen: &Screen) {
        self.dirty = false;
        self.matches.clear();
        self.error = None;
        if self.query.is_empty() {
            self.current = None;
            return;
        }
        let insensitive = self.effective_insensitive();
        if self.regex {
            self.scan_regex(screen, insensitive);
        } else {
            self.scan_literal(screen, insensitive);
        }
        self.current = if self.matches.is_empty() {
            None
        } else {
            Some(self.current.unwrap_or(0).min(self.matches.len() - 1))
        };
    }

    /// Matches intersecting `row`, each as `(col, len, is_current)` — the
    /// widget's highlight source.
    pub fn row_highlights(&self, row: usize) -> impl Iterator<Item = (usize, usize, bool)> + '_ {
        self.matches
            .iter()
            .enumerate()
            .filter(move |(_, m)| m.row == row)
            .map(move |(i, m)| (m.col, m.len, self.current == Some(i)))
    }

    /// Whether the effective match is case-insensitive for the current query.
    fn effective_insensitive(&self) -> bool {
        match self.case {
            CaseMode::Sensitive => false,
            CaseMode::Insensitive => true,
            CaseMode::Smart => !self.query.chars().any(char::is_uppercase),
        }
    }

    fn scan_regex(&mut self, screen: &Screen, insensitive: bool) {
        let re = match RegexBuilder::new(&self.query)
            .case_insensitive(insensitive)
            .build()
        {
            Ok(re) => re,
            Err(err) => {
                self.error = Some(err.to_string());
                self.current = None;
                return;
            }
        };
        for row in 0..screen.rows() {
            let text = screen.line_text(row);
            for m in re.find_iter(&text) {
                if m.start() == m.end() {
                    continue; // a zero-width match highlights nothing.
                }
                let col = text[..m.start()].chars().count();
                let len = text[m.start()..m.end()].chars().count();
                self.matches.push(Match { row, col, len });
            }
        }
    }

    fn scan_literal(&mut self, screen: &Screen, insensitive: bool) {
        let needle: Vec<char> = self.query.chars().collect();
        for row in 0..screen.rows() {
            let hay: Vec<char> = screen.line_text(row).chars().collect();
            for col in find_literal(&hay, &needle, insensitive) {
                self.matches.push(Match {
                    row,
                    col,
                    len: needle.len(),
                });
            }
        }
    }
}

/// Non-overlapping start columns where `needle` occurs in `hay`.
///
/// ASCII-case-insensitive when asked (the terminal-search convention); an empty
/// needle matches nothing.
fn find_literal(hay: &[char], needle: &[char], insensitive: bool) -> Vec<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let eq = |a: char, b: char| {
        if insensitive {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    };
    let mut hits = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if needle.iter().enumerate().all(|(k, &n)| eq(hay[i + k], n)) {
            hits.push(i);
            i += needle.len(); // non-overlapping.
        } else {
            i += 1;
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Terminal;

    /// A screen fed `bytes` on a `cols × rows` grid with room for history.
    fn screen(cols: usize, rows: usize, bytes: &[u8]) -> Screen {
        let mut term = Terminal::new(cols, rows, 1000);
        term.feed(bytes);
        term.full()
    }

    #[test]
    fn literal_find_lists_every_row_hit_in_order() {
        let mut s = Search::new();
        s.set_query("cat");
        s.recompute(&screen(20, 3, b"cat dog cat\r\nno match\r\ncatcat"));
        // Row 0: two "cat"s; row 2: two more (non-overlapping).
        let rows: Vec<usize> = s.matches().iter().map(|m| m.row).collect();
        assert_eq!(rows, vec![0, 0, 2, 2]);
        assert_eq!(
            s.matches()[0],
            Match {
                row: 0,
                col: 0,
                len: 3
            }
        );
        assert_eq!(
            s.matches()[1],
            Match {
                row: 0,
                col: 8,
                len: 3
            }
        );
        assert_eq!(
            s.matches()[2],
            Match {
                row: 2,
                col: 0,
                len: 3
            }
        );
        assert_eq!(
            s.matches()[3],
            Match {
                row: 2,
                col: 3,
                len: 3
            }
        );
        assert_eq!(s.count(), 4);
    }

    #[test]
    fn empty_query_finds_nothing() {
        let mut s = Search::new();
        s.recompute(&screen(10, 2, b"anything"));
        assert_eq!(s.count(), 0);
        assert_eq!(s.current_match(), None);
    }

    #[test]
    fn smart_case_folds_until_the_query_has_an_uppercase() {
        let scr = screen(20, 2, b"Foo foo FOO");
        let mut s = Search::new();
        // Smart + all-lowercase query → case-insensitive (all three hit).
        s.set_query("foo");
        s.recompute(&scr);
        assert_eq!(s.count(), 3);
        // Smart + an uppercase in the query → case-sensitive (only "Foo").
        s.set_query("Foo");
        s.recompute(&scr);
        assert_eq!(s.count(), 1);
        assert_eq!(s.matches()[0].col, 0);
    }

    #[test]
    fn explicit_case_modes_override_smart() {
        let scr = screen(20, 1, b"Foo foo FOO");
        let mut s = Search::new();
        s.set_query("FOO");
        s.cycle_case(); // Smart → Sensitive
        assert_eq!(s.case(), CaseMode::Sensitive);
        s.recompute(&scr);
        assert_eq!(s.count(), 1); // only the literal "FOO"
        s.cycle_case(); // Sensitive → Insensitive
        s.recompute(&scr);
        assert_eq!(s.count(), 3);
    }

    #[test]
    fn regex_query_matches_and_reports_columns() {
        let mut s = Search::new();
        s.toggle_regex();
        assert!(s.is_regex());
        s.set_query(r"\d+");
        s.recompute(&screen(20, 2, b"err 42 at 7\r\nok"));
        let cols: Vec<(usize, usize, usize)> =
            s.matches().iter().map(|m| (m.row, m.col, m.len)).collect();
        assert_eq!(cols, vec![(0, 4, 2), (0, 10, 1)]);
    }

    #[test]
    fn invalid_regex_is_an_honest_error_not_a_panic() {
        let mut s = Search::new();
        s.toggle_regex();
        s.set_query("(unclosed");
        s.recompute(&screen(10, 1, b"unclosed"));
        assert!(s.error().is_some(), "invalid regex records a message");
        assert_eq!(s.count(), 0);
        assert_eq!(s.current_match(), None);
    }

    #[test]
    fn zero_width_regex_matches_are_skipped() {
        let mut s = Search::new();
        s.toggle_regex();
        s.set_query("a*"); // matches the empty string everywhere
        s.recompute(&screen(10, 1, b"aa bb"));
        // Only the real "aa" run counts; the empty matches are dropped.
        assert_eq!(s.count(), 1);
        assert_eq!(
            s.matches()[0],
            Match {
                row: 0,
                col: 0,
                len: 2
            }
        );
    }

    #[test]
    fn next_and_prev_wrap_around() {
        let mut s = Search::new();
        s.set_query("x");
        s.recompute(&screen(10, 1, b"x x x"));
        assert_eq!(s.count(), 3);
        // recompute seeds current at the first match.
        assert_eq!(s.current_index(), Some(0));
        s.next_match();
        assert_eq!(s.current_index(), Some(1));
        s.next_match();
        s.next_match(); // 2 → wrap to 0
        assert_eq!(s.current_index(), Some(0));
        s.prev_match(); // 0 → wrap to 2
        assert_eq!(s.current_index(), Some(2));
        assert_eq!(s.current_row(), Some(0));
    }

    #[test]
    fn recompute_clamps_a_stale_current_index() {
        let mut s = Search::new();
        s.set_query("x");
        s.recompute(&screen(10, 1, b"x x x"));
        s.next_match();
        s.next_match(); // current = 2
                        // A narrower screen now yields fewer matches — current clamps in-range.
        s.recompute(&screen(10, 1, b"x"));
        assert_eq!(s.count(), 1);
        assert_eq!(s.current_index(), Some(0));
    }

    #[test]
    fn row_highlights_flag_the_current_match() {
        let mut s = Search::new();
        s.set_query("ab");
        s.recompute(&screen(10, 1, b"ab ab"));
        s.next_match(); // current is the second "ab"
        let hl: Vec<(usize, usize, bool)> = s.row_highlights(0).collect();
        assert_eq!(hl, vec![(0, 2, false), (3, 2, true)]);
        // A row with no matches yields nothing.
        assert_eq!(s.row_highlights(5).count(), 0);
    }

    #[test]
    fn open_close_toggle_track_active_and_schedule_a_rescan() {
        let mut s = Search::new();
        assert!(!s.active());
        s.toggle();
        assert!(s.active() && s.dirty());
        s.recompute(&screen(4, 1, b"z"));
        assert!(!s.dirty(), "recompute clears the dirty flag");
        s.push_str("z");
        assert!(s.dirty(), "typing schedules a rescan");
        s.toggle();
        assert!(!s.active(), "query kept, overlay closed");
    }
}
