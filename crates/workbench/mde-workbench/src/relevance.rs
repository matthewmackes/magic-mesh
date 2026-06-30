//! CTRLSURF-1 — the unified verb-aware relevance ladder.
//!
//! ONE scoring path behind every Workbench search, so the app launcher overlay
//! and the Front Door omnibox rank identically and there is a single place to
//! tune relevance. It merges the two ladders that had drifted apart — the
//! omnibox's `score_match` (exact / prefix / word-prefix / substring) and the
//! launcher's `fuzzy_score` (the same four tiers plus a typo-tolerant
//! subsequence tail) — into the richer superset of the two:
//!
//! - a fuzzy hit never outranks a real substring (the tail sits strictly below
//!   tier 40), so the merge does not surface mid-word noise above a prefix hit;
//! - the word-prefix tier is delimiter-aware across whitespace `-` `_` `/` `.`,
//!   so `org.gimp.GIMP` answers to `gimp` as a word, not a bare substring.
//!
//! Pure + case-insensitive: no view, no Bus, directly unit-testable. The Front
//! Door score tests (`panels::front_door`) drive this engine unchanged through
//! the `search::score_match` and `launcher::fuzzy_score` re-exports, which now
//! both point here — so "same input, same rank" is enforced by construction.

/// Score one candidate `haystack` against `needle`. Higher is better; `0` means
/// no match (the candidate is dropped). The ladder, best-first:
///
/// - **100** — exact (case-insensitive) equality;
/// - **80** — the haystack starts with the needle;
/// - **60** — the needle starts a delimiter-bounded word (whitespace / `-` /
///   `_` / `/` / `.`);
/// - **40** — a bare interior substring;
/// - **1..=30** — a fuzzy in-order subsequence (typo / abbreviation tolerant),
///   scaled by how tight the matched span is, and always kept below a real
///   substring so an exact-ish hit can never lose to a scattered one.
///
/// Pure + case-insensitive.
#[must_use]
pub fn score(haystack: &str, needle: &str) -> u32 {
    if needle.is_empty() {
        return 0;
    }
    let hay = haystack.to_lowercase();
    let need = needle.to_lowercase();
    if hay == need {
        return 100;
    }
    if hay.starts_with(&need) {
        return 80;
    }
    // A match at the start of any whitespace/`-`/`_`/`/`/`.`-delimited word.
    if hay
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '/' || c == '.')
        .any(|word| word.starts_with(&need))
    {
        return 60;
    }
    if hay.contains(&need) {
        return 40;
    }
    // Fuzzy tail: an in-order subsequence match (the needle's chars appear in
    // order in the haystack, gaps allowed) — typo/abbreviation tolerant. The
    // score rewards a tighter span (fewer gap chars between the matched ones),
    // so "frefox" beats a loose scatter. Below substring so a real substring
    // always outranks a fuzzy hit.
    match subsequence_span(&hay, &need) {
        Some(span) if span > 0 => {
            // Tightness in [0,1]: needle-len / matched-span. Map to 1..=30.
            let tightness = (need.chars().count() as f32) / (span as f32);
            1 + (tightness * 29.0).round() as u32
        }
        _ => 0,
    }
}

/// The character span (first..=last matched index, inclusive count) of the
/// tightest in-order subsequence of `needle` within `haystack`, or `None` when
/// `needle` isn't a subsequence. A greedy left-to-right scan finds the first
/// occurrence of each needle char after the previous; the span is the index
/// distance covered. Operates on `char`s (Unicode-safe). Returns the matched
/// span length in chars (>= needle length).
#[must_use]
fn subsequence_span(haystack: &str, needle: &str) -> Option<usize> {
    let hay: Vec<char> = haystack.chars().collect();
    let need: Vec<char> = needle.chars().collect();
    if need.is_empty() {
        return None;
    }
    let mut hi = 0usize;
    let mut first = None;
    let mut last = 0usize;
    for &nc in &need {
        let mut matched = false;
        while hi < hay.len() {
            if hay[hi] == nc {
                if first.is_none() {
                    first = Some(hi);
                }
                last = hi;
                hi += 1;
                matched = true;
                break;
            }
            hi += 1;
        }
        if !matched {
            return None;
        }
    }
    first.map(|f| last - f + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_orders_exact_prefix_word_substring_then_fuzzy() {
        // The four hard tiers, top to bottom (the former `score_match` ladder).
        assert_eq!(score("Build Farm", "build farm"), 100);
        assert_eq!(score("Build Farm", "build"), 80);
        // "farm" is a word-boundary prefix (after the space), not a leading one.
        assert_eq!(score("Build Farm", "farm"), 60);
        assert!(score("Build Farm", "build") > score("Build Farm", "farm"));
        // A bare interior substring is the weakest real match.
        assert_eq!(score("Datacenter", "cent"), 40);
        // No match / empty needle → 0 (dropped).
        assert_eq!(score("Mesh", "zzz"), 0);
        assert_eq!(score("Mesh", ""), 0);
    }

    #[test]
    fn fuzzy_tail_matches_typos_and_abbreviations_below_substring() {
        // A dropped letter still matches via the subsequence tail, but strictly
        // below a real substring (so a substring hit always wins).
        let typo = score("Firefox", "frefox");
        assert!(typo > 0 && typo < 40, "fuzzy below substring: {typo}");
        // An abbreviation (non-contiguous chars in order) matches.
        assert!(score("GIMP", "gmp") > 0);
        // A tighter subsequence outscores a looser one.
        assert!(score("abcde", "ace") > score("axxxbxxxce", "ace"));
    }

    #[test]
    fn word_prefix_tier_is_dot_aware() {
        // The merge folded the launcher's `.` delimiter into the word-prefix
        // tier, so a dotted id answers to a whole segment as a word (60), not a
        // bare substring (40).
        assert_eq!(score("org.gimp.GIMP", "gimp"), 60);
    }
}
