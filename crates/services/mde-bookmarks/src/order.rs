//! LSEQ-style fractional-index order keys (lock Q3).
//!
//! Manual ordering is a **fractional index**: each item carries an opaque string
//! `order_key`, and siblings render sorted by plain lexicographic comparison of
//! those keys. To drag an item between two neighbours the worker mints a key
//! *strictly between* their two keys with [`key_between`] — one op, no renumber
//! storm across the rest of the list.
//!
//! Keys are digit strings over a fixed 62-symbol alphabet whose symbols are in
//! ascending byte order, so lexicographic **byte** comparison of two keys equals
//! comparison of their fractional values. The generator never emits a key ending
//! in the minimum digit, which is exactly the condition that keeps byte order and
//! fractional order in agreement (a trailing-minimum suffix is the one case where
//! "shorter string sorts first" would disagree with "pad-with-zero" value order).

/// The ordered key alphabet: `0-9 A-Z a-z`, already in ascending ASCII order, so
/// digit-index order equals byte order. 62 symbols.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Radix = the alphabet length. Used as the "one past the maximum digit"
/// sentinel for an unbounded upper neighbour.
const RADIX: usize = ALPHABET.len();

/// Decode a key string into digit indices. An out-of-alphabet byte maps to 0
/// (the minimum digit) so a corrupt key still yields a usable lower bound rather
/// than panicking — this crate never panics on peer-supplied data.
fn decode(key: &str) -> Vec<usize> {
    key.bytes()
        .map(|b| ALPHABET.iter().position(|&c| c == b).unwrap_or(0))
        .collect()
}

/// Encode digit indices back into a key string.
fn encode(digits: &[usize]) -> String {
    // Every digit index is < RADIX by construction, so indexing is in-bounds.
    let bytes: Vec<u8> = digits.iter().map(|&d| ALPHABET[d]).collect();
    // The alphabet is ASCII, so this is always valid UTF-8.
    String::from_utf8(bytes).unwrap_or_default()
}

/// Emit a digit sequence strictly between `lo` and `hi`, where a missing `hi`
/// (`None`) means "unbounded above". Precondition: `lo` sorts strictly before
/// `hi` under pad-with-zero comparison. The result never ends in digit 0.
fn between(lo: &[usize], hi: Option<&[usize]>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let d_lo = lo.get(i).copied().unwrap_or(0);
        let d_hi = hi.map_or(RADIX, |h| h.get(i).copied().unwrap_or(0));
        if d_lo == d_hi {
            out.push(d_lo);
            i += 1;
            continue;
        }
        // Precondition guarantees d_lo < d_hi here.
        if d_hi - d_lo >= 2 {
            out.push(d_lo + (d_hi - d_lo) / 2);
            return out;
        }
        // Neighbouring digits with no gap: keep lo's digit (staying < hi) and
        // descend into lo's tail with an unbounded upper bound.
        out.push(d_lo);
        let tail_start = (i + 1).min(lo.len());
        let mut tail = between(&lo[tail_start..], None);
        out.append(&mut tail);
        return out;
    }
}

/// Mint an order key strictly between the two neighbouring siblings' keys.
///
/// `before` is the key of the sibling that should sort *before* the new item
/// (`None` = insert at the head); `after` is the sibling that should sort
/// *after* it (`None` = insert at the tail). With both `None` the list is empty
/// and a stable middle key is returned.
///
/// Guarantees `before < result < after` under lexicographic byte comparison
/// whenever `before < after`. If the two bounds are equal or inverted (a
/// caller bug) the lower bound is used for both, still yielding a key that sorts
/// after `before`.
#[must_use]
pub fn key_between(before: Option<&str>, after: Option<&str>) -> String {
    let lo = before.map(decode).unwrap_or_default();
    let hi = after.and_then(|a| {
        let hi = decode(a);
        // Guard against an inverted/equal bound: fall back to unbounded.
        (lo < hi).then_some(hi)
    });
    encode(&between(&lo, hi.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_strictly_between(before: Option<&str>, after: Option<&str>) -> String {
        let mid = key_between(before, after);
        if let Some(b) = before {
            assert!(b < mid.as_str(), "{b:?} < {mid:?}");
        }
        if let Some(a) = after {
            assert!(mid.as_str() < a, "{mid:?} < {a:?}");
        }
        assert!(!mid.is_empty(), "key is never empty");
        assert!(
            !mid.ends_with('0'),
            "key {mid:?} must not end in the minimum digit"
        );
        mid
    }

    #[test]
    fn first_key_in_an_empty_list() {
        assert_strictly_between(None, None);
    }

    #[test]
    fn append_and_prepend() {
        let first = assert_strictly_between(None, None);
        assert_strictly_between(Some(&first), None); // append after
        assert_strictly_between(None, Some(&first)); // prepend before
    }

    #[test]
    fn insert_between_adjacent_keys() {
        let a = key_between(None, None);
        let b = key_between(Some(&a), None);
        // The classic tight case: wedge a key between two neighbours.
        assert_strictly_between(Some(&a), Some(&b));
    }

    #[test]
    fn repeated_head_inserts_stay_ordered() {
        // Keep inserting at the head; each new key must sort before the last.
        let mut keys = vec![key_between(None, None)];
        for _ in 0..64 {
            let head = keys.last().cloned();
            let k = key_between(None, head.as_deref());
            if let Some(prev) = head {
                assert!(k < prev, "{k:?} < {prev:?}");
            }
            keys.push(k);
        }
    }

    #[test]
    fn deep_repeated_between_inserts_stay_ordered() {
        // Repeatedly wedge between the same two anchors; each new key must land
        // strictly between the previous wedge and the upper anchor, exercising
        // the digit-descent path many levels deep.
        let lo = key_between(None, None);
        let hi = key_between(Some(&lo), None);
        let mut cur = lo;
        for _ in 0..64 {
            let next = key_between(Some(&cur), Some(&hi));
            assert!(cur < next && next < hi, "{cur:?} < {next:?} < {hi:?}");
            cur = next;
        }
    }
}
