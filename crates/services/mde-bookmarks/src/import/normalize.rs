//! URL normalization for the dedup key (lock Q15).
//!
//! Two bookmarks are "the same link" if their URLs normalize equal: lowercase
//! host, no fragment, no tracking query params, the remaining query sorted, and
//! no trailing slash. The normalized form is used ONLY as a dedup key — the
//! bookmark always stores the browser's original URL.

use url::Url;

/// Query-parameter keys stripped as tracking noise before dedup (lock Q15). Any
/// `utm_*` key is stripped in addition to these.
const TRACKING_KEYS: &[&str] = &[
    "gclid", "fbclid", "msclkid", "dclid", "gbraid", "wbraid", "yclid", "mc_eid", "mc_cid",
    "igshid", "_ga", "_gl", "ref_src", "ref_url", "spm", "scm",
];

/// Whether a query-parameter key is a tracking param to be dropped.
fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_KEYS.contains(&k.as_str())
}

/// Normalize a URL into a stable dedup key (lock Q15).
///
/// Lowercase host, drop the fragment, strip tracking query params, sort the
/// remaining query, and trim a trailing slash. Returns `None` for input that
/// does not parse as a URL.
#[must_use]
pub fn normalize_url(raw: &str) -> Option<String> {
    let mut u = Url::parse(raw.trim()).ok()?;
    u.set_fragment(None);

    // Filter + sort the query so the key is order-independent.
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        u.set_query(None);
    } else {
        let mut kept = kept;
        kept.sort();
        let mut q = u.query_pairs_mut();
        q.clear();
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
    }

    // `url` already lowercases the host for special schemes; trim a single
    // trailing slash so `https://a.com/` and `https://a.com` dedup together.
    let key = u.as_str().trim_end_matches('/');
    Some(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn strips_fragment_and_trailing_slash() {
        assert_eq!(
            normalize_url("https://Example.com/path/#section"),
            normalize_url("https://example.com/path")
        );
    }

    #[test]
    fn strips_tracking_params_but_keeps_real_ones() {
        let n = normalize_url("https://a.example/p?utm_source=x&id=7&fbclid=zz").expect("parses");
        assert_eq!(n, "https://a.example/p?id=7");
    }

    #[test]
    fn query_order_is_canonical() {
        assert_eq!(
            normalize_url("https://a.example/?b=2&a=1"),
            normalize_url("https://a.example/?a=1&b=2")
        );
    }

    #[test]
    fn junk_returns_none() {
        assert_eq!(normalize_url("not a url"), None);
    }
}
