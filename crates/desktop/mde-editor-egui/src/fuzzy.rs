//! EDITOR-7 — a tiny, **self-contained** fuzzy matcher for the file-finder and
//! command-palette overlays.
//!
//! Deliberately local to this crate: it does NOT reach for the shell's store or
//! any other matcher — a subsequence match with a small, legible score (boundary
//! and contiguity bonuses, gap and leading penalties) so a query ranks the
//! obvious hit first. The scoring is pure data-in / data-out (no egui), so it is
//! unit-tested directly, and both overlays rank through the one [`ranked`] seam.

// `missing_const_for_fn` (nursery) is over-eager on the small helpers here; keeping
// them plain functions matches the crate's other modules and pins nothing into a
// `const` contract we don't want.
#![allow(clippy::missing_const_for_fn)]

/// Score awarded when a matched char sits on a word boundary (a path separator,
/// `_` / `-` / `.` / space, or a camel-case hump) — the humps a human aims at.
const BOUNDARY_BONUS: i32 = 16;
/// Score awarded when a matched char immediately follows the previous match — a
/// contiguous run of the query (e.g. `main` inside `main.rs`).
const CONTIGUOUS_BONUS: i32 = 8;
/// Bonus when the matched char has the same case as the query char, so an
/// exact-case hit edges out a case-folded one.
const CASE_BONUS: i32 = 2;
/// Penalty per skipped char in a gap between two matches (capped), so a tightly
/// packed match outranks a scattered one.
const GAP_PENALTY: i32 = -1;
/// Penalty per leading unmatched char before the first match (capped), so a match
/// near the start outranks one buried deep in the string.
const LEADING_PENALTY: i32 = -1;
/// Cap on the per-match gap / leading penalty so one very long string can't
/// dominate the score with penalties alone.
const PENALTY_CAP: usize = 12;

/// Case-insensitive (ASCII-fold) char equality — file paths + command titles are
/// ASCII-dominant, so an ASCII fold is enough and stays allocation-free.
fn eq_ci(a: char, b: char) -> bool {
    a.eq_ignore_ascii_case(&b)
}

/// Whether `cur` begins a new "word" given the char `prev` immediately before it —
/// a path-separator / punctuation break, or a camel-case hump (lower → UPPER).
fn is_boundary(prev: char, cur: char) -> bool {
    let separator = matches!(prev, '/' | '\\' | '_' | '-' | '.' | ' ');
    let camel_hump = !prev.is_uppercase() && cur.is_uppercase();
    separator || camel_hump
}

/// The penalty count for a `gap` of skipped chars, capped so a single long span
/// can't swamp the score.
fn capped(gap: usize) -> i32 {
    i32::try_from(gap.min(PENALTY_CAP)).unwrap_or(0)
}

/// Score `needle` against `haystack`, or `None` when `needle` is not an in-order,
/// case-insensitive subsequence of `haystack`. Higher is better.
///
/// An empty needle scores a neutral `0` (everything matches), so an empty query
/// lists every candidate. The match is greedy left-to-right — the standard,
/// lightweight fuzzy-finder heuristic — with a boundary bonus, a contiguity bonus,
/// and gap / leading penalties folded in.
#[must_use]
pub fn score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    let ndl: Vec<char> = needle.chars().collect();

    let mut total: i32 = 0;
    let mut ni = 0usize;
    let mut prev: Option<usize> = None;

    for (hi, &hc) in hay.iter().enumerate() {
        let Some(&nc) = ndl.get(ni) else { break };
        if !eq_ci(hc, nc) {
            continue;
        }
        // Position score: contiguous run, a gap after a prior match, or the run-in
        // before the first match. Exactly one of the three applies per matched char.
        total += match prev {
            Some(p) if p + 1 == hi => CONTIGUOUS_BONUS,
            Some(p) => GAP_PENALTY * capped(hi - p - 1),
            None => LEADING_PENALTY * capped(hi),
        };
        if hi == 0 || is_boundary(hay[hi - 1], hc) {
            total += BOUNDARY_BONUS;
        }
        if hc == nc {
            total += CASE_BONUS;
        }
        prev = Some(hi);
        ni += 1;
    }

    (ni == ndl.len()).then_some(total)
}

/// Rank `candidates` (each an `(index, text)`) against `needle`, best score first,
/// dropping the ones that don't match. Ties keep input order (a stable sort), so
/// the ranking is deterministic. Returns the surviving indices in ranked order —
/// the single seam both overlays filter their lists through.
#[must_use]
pub fn ranked<'a, I>(needle: &str, candidates: I) -> Vec<usize>
where
    I: IntoIterator<Item = (usize, &'a str)>,
{
    let mut scored: Vec<(usize, i32)> = candidates
        .into_iter()
        .filter_map(|(idx, text)| score(needle, text).map(|s| (idx, s)))
        .collect();
    // Stable: equal scores keep their input order, so ties are deterministic.
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

#[cfg(test)]
mod tests {
    use super::{ranked, score};

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(score("xz", "abc"), None, "chars not present → no match");
        assert_eq!(score("cba", "abc"), None, "out-of-order → no match");
        assert_eq!(
            score("abcd", "abc"),
            None,
            "needle longer than a hit → none"
        );
    }

    #[test]
    fn empty_needle_matches_everything_neutrally() {
        assert_eq!(score("", "anything"), Some(0));
        let all = ranked("", ["a", "b", "c"].iter().copied().enumerate());
        assert_eq!(
            all,
            vec![0, 1, 2],
            "an empty query lists every candidate in order"
        );
    }

    #[test]
    fn boundary_and_contiguity_rank_the_obvious_hit_first() {
        // "a_b_c" (each hit on a '_' boundary) beats "abcdef" (one boundary, then a
        // contiguous run) beats "xaxbxc" (scattered, no boundaries).
        let cands = ["abcdef", "a_b_c", "xaxbxc"];
        let out = ranked("abc", cands.iter().copied().enumerate());
        assert_eq!(
            out,
            vec![1, 0, 2],
            "boundary-dense > contiguous > scattered"
        );
    }

    #[test]
    fn a_contiguous_filename_hit_outranks_a_scattered_one() {
        // The finder's bread-and-butter: "main" should rank the file whose name IS
        // "main.rs" above one where the letters are merely a subsequence.
        let cands = ["src/domain_view.rs", "src/main.rs"];
        let out = ranked("main", cands.iter().copied().enumerate());
        assert_eq!(out.first(), Some(&1), "main.rs wins for the query 'main'");
        assert_eq!(out.len(), 2, "both are subsequence matches");
    }

    #[test]
    fn ranked_drops_non_matches_entirely() {
        let cands = ["alpha", "beta", "gamma"];
        assert!(
            ranked("zzz", cands.iter().copied().enumerate()).is_empty(),
            "a query matching nothing yields no results"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(
            score("MAIN", "main.rs").is_some(),
            "upper query matches lower text"
        );
        assert!(
            score("main", "MAIN.RS").is_some(),
            "lower query matches upper text"
        );
    }
}
