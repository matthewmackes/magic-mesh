//! Pure-fn helpers for encoding panel values into the JSON
//! shapes `dev.mackes.MDE.Settings.Set` expects, and decoding
//! the matching `Get` responses back into Rust values.
//!
//! Lifted out of the per-panel modules once the fifth panel
//! (CB-1.4 removable) needed the same shape — keeping the
//! duplication grew the bug-fix surface beyond what the
//! "three similar lines is better than abstraction" rule
//! tolerates.

/// JSON-encode a string by wrapping in quotes + escaping
/// backslashes / quotes (covers the values the Phase C
/// appliers see in practice — no embedded control chars).
#[must_use]
pub fn quote_json(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Inverse of [`quote_json`] — strip surrounding `"…"` if
/// present, unescape `\"` and `\\`. Empty / unquoted input
/// passes through unchanged so the panel's `Get` value never
/// rejects on shape alone.
#[must_use]
pub fn strip_json_quotes(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    }
}

/// JSON boolean parser. Accepts canonical `true` / `false`,
/// `"true"` / `"false"`, and legacy integer `1` / `0` /
/// `"yes"` / `"no"` (sidecars from earlier wiring may carry
/// the older int form).
#[must_use]
pub fn parse_bool(s: &str) -> bool {
    let t = s.trim().trim_matches('"').to_ascii_lowercase();
    matches!(t.as_str(), "true" | "1" | "yes")
}

/// Encode a Rust bool as the canonical JSON keyword
/// (`true` / `false`). The Phase C appliers always read the
/// canonical form after a Set so this is one-way.
#[must_use]
pub const fn encode_bool(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

/// Parse a `u32` from a panel's text input. Strips
/// surrounding quotes (Get may return `"3000"` or `3000`),
/// returns `None` on non-numeric input.
#[must_use]
pub fn parse_u32(s: &str) -> Option<u32> {
    let trimmed = s.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_round_trips_plain_string() {
        let s = "Adwaita-dark";
        assert_eq!(strip_json_quotes(&quote_json(s)), s);
    }

    #[test]
    fn quote_round_trips_string_with_embedded_quotes() {
        let s = "weird \"name\"";
        assert_eq!(strip_json_quotes(&quote_json(s)), s);
    }

    #[test]
    fn strip_json_quotes_passes_through_unquoted_input() {
        assert_eq!(strip_json_quotes("plain"), "plain");
    }

    #[test]
    fn strip_json_quotes_handles_empty() {
        assert_eq!(strip_json_quotes(""), "");
        assert_eq!(strip_json_quotes("\"\""), "");
    }

    #[test]
    fn parse_bool_accepts_canonical_and_legacy_forms() {
        assert!(parse_bool("true"));
        assert!(parse_bool("\"true\""));
        assert!(parse_bool("1"));
        assert!(parse_bool("yes"));
        assert!(!parse_bool("false"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool(""));
        assert!(!parse_bool("maybe"));
    }

    #[test]
    fn encode_bool_emits_json_keywords() {
        assert_eq!(encode_bool(true), "true");
        assert_eq!(encode_bool(false), "false");
    }

    #[test]
    fn parse_u32_accepts_plain_and_quoted_integers() {
        assert_eq!(parse_u32("3000"), Some(3000));
        assert_eq!(parse_u32("\"3000\""), Some(3000));
        assert_eq!(parse_u32("0"), Some(0));
    }

    #[test]
    fn parse_u32_rejects_empty_or_non_numeric() {
        assert_eq!(parse_u32(""), None);
        assert_eq!(parse_u32("forever"), None);
        assert_eq!(parse_u32("-100"), None);
    }
}
