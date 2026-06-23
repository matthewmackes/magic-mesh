//! CLIP-VIEW-1 — the Notification Hub's **Clipboard Viewer** model.
//!
//! The render-agnostic half of the Hub's clipboard section: the row model,
//! the `action/clipboard/list` reply parser, and the relative-age formatter.
//! All pure + unit-tested in here; the bin (`mde-notify-center`) reads these
//! and draws the themed rows + wires the `list/pin/unpin/delete/clear` Bus
//! verbs (CLIP-SYNC-1, `mackesd::ipc::clipboard`).
//!
//! The mesh-global history is one shared `clipboard/history.json` the
//! daemon owns; the viewer never touches the file directly — it lists +
//! mutates through the Bus so every edit is mesh-wide (delete/pin/clear hit
//! the same shared document the capture worker appends to). Click-to-load
//! is the one *local* action: it `wl-copy`s the row text onto THIS node's
//! Wayland clipboard (the capture worker then re-syncs / debounces it).
//!
//! Time handling: each entry carries an RFC3339 capture stamp (CLIP-SYNC-1
//! O6). The workbench crate has no `chrono` (and must not link the mesh
//! crate that owns the daemon's `age_label`), so [`rfc3339_to_epoch`] parses
//! the fixed `YYYY-MM-DDTHH:MM:SS[.fff][±HH:MM|Z]` shape `chrono::to_rfc3339`
//! emits into epoch-seconds itself. The bin then feeds that × 1000 into its
//! own shared `format_age` ladder so the clipboard ages read off the SAME
//! "Ns / Nm / Nh / Nd" bucket vocabulary as the notifications list (one
//! ladder, no duplicate).

use serde::Deserialize;

/// One clipboard entry as rendered in the viewer. Mirrors the daemon's
/// `ClipEntry` (`mackesd::workers::clipboard_sync::ClipEntry`) but is a local
/// decode of the `action/clipboard/list` reply — the viewer never links the
/// mesh crate, it only speaks the Bus JSON.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClipRow {
    /// Stable content-fingerprint id — addresses the entry for pin/delete.
    pub id: String,
    /// The clip text (verbatim).
    pub text: String,
    /// Node that captured the clip (O6 source attribution).
    #[serde(default)]
    pub source: String,
    /// RFC3339 capture timestamp (O6).
    #[serde(default)]
    pub time: String,
    /// Pinned entries are exempt from the 50-cap + survive a clear-all.
    #[serde(default)]
    pub pinned: bool,
}

/// Parse an `action/clipboard/list` reply body into the viewer's rows.
///
/// The reply envelope is `{ "ok": true, "entries": [ClipEntry,…] }` (newest
/// first); an `{ "error": … }` envelope, a missing `entries`, or any decode
/// failure yields an empty list — the section then renders its honest empty
/// state rather than surfacing a parse panic. Pure + testable.
#[must_use]
pub fn parse_list_reply(body: &str) -> Vec<ClipRow> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(entries) = v.get("entries").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| serde_json::from_value::<ClipRow>(e.clone()).ok())
        .collect()
}

/// A single-line preview of a clip for the row: collapse runs of whitespace
/// (newlines/tabs) to single spaces so a multi-line clip stays on one row,
/// then truncate to `max` chars with an ellipsis. Pure + testable.
#[must_use]
pub fn preview(text: &str, max: usize) -> String {
    let flat: String = {
        let mut out = String::with_capacity(text.len());
        let mut in_ws = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !in_ws {
                    out.push(' ');
                    in_ws = true;
                }
            } else {
                out.push(ch);
                in_ws = false;
            }
        }
        out.trim().to_string()
    };
    if flat.chars().count() <= max {
        flat
    } else {
        let head: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{head}\u{2026}") // …
    }
}

/// Parse the date/time fields of an RFC3339 stamp into epoch **seconds**
/// (UTC). Handles the `chrono::to_rfc3339` output shape:
/// `YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM]`. Returns `None` for anything
/// that doesn't match (the caller then renders "now"). The fractional part is
/// ignored (second resolution is all the age buckets need); the offset is
/// applied so a non-UTC stamp still lands on the right second.
#[must_use]
pub fn rfc3339_to_epoch(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    // Need at least "YYYY-MM-DDTHH:MM:SS".
    if bytes.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> { stamp.get(a..b)?.parse::<i64>().ok() };
    // Positional parse with the literal separators checked.
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ' {
        return None;
    }
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let min = num(14, 16)?;
    let sec = num(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let mut epoch = days * 86_400 + hour * 3600 + min * 60 + sec;
    // Trailing zone: scan past an optional ".fraction" to the Z / ±HH:MM.
    let mut rest = &stamp[19..];
    if let Some(stripped) = rest.strip_prefix('.') {
        let nondigit = stripped
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(stripped.len());
        rest = &stripped[nondigit..];
    }
    // Apply the offset so the epoch is true UTC. "Z"/empty = UTC (no shift).
    if let Some(sign) = rest.chars().next() {
        if sign == '+' || sign == '-' {
            if let (Some(oh), Some(om)) = (
                rest.get(1..3).and_then(|s| s.parse::<i64>().ok()),
                rest.get(4..6).and_then(|s| s.parse::<i64>().ok()),
            ) {
                let off = oh * 3600 + om * 60;
                // A +05:00 local stamp is 5h *ahead* of UTC → subtract to get UTC.
                epoch += if sign == '-' { off } else { -off };
            }
        }
    }
    Some(epoch)
}

/// Days since the Unix epoch (1970-01-01) for a civil Y-M-D, by Howard
/// Hinnant's `days_from_civil` (proleptic Gregorian, valid for all the
/// dates a clipboard stamp can carry). Pure integer math — no chrono.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The "from <node> · <age>" sub-label for a row. `age` is pre-formatted by
/// the caller (the bin feeds [`rfc3339_to_epoch`] × 1000 into its shared
/// `format_age` ladder, so the viewer's ages read identically to the
/// notifications list off ONE bucket ladder, not a duplicate). An empty
/// source (a pre-O6 entry) collapses to just the age. Pure + testable.
#[must_use]
pub fn meta_label(source: &str, age: &str) -> String {
    if source.is_empty() {
        age.to_string()
    } else {
        format!("{source} \u{00b7} {age}") // node · age
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_reply_decodes_entries_newest_first() {
        let body = r#"{"ok":true,"entries":[
            {"id":"aa","text":"newest","source":"n1","time":"2026-06-21T12:00:00+00:00","pinned":true},
            {"id":"bb","text":"older","source":"n2","time":"2026-06-21T11:00:00+00:00"}
        ]}"#;
        let rows = parse_list_reply(body);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "aa");
        assert_eq!(rows[0].text, "newest");
        assert!(rows[0].pinned);
        // pinned defaults to false when omitted.
        assert!(!rows[1].pinned);
        assert_eq!(rows[1].source, "n2");
    }

    #[test]
    fn parse_list_reply_error_or_garbage_is_empty_not_panic() {
        assert!(parse_list_reply(r#"{"error":"boom"}"#).is_empty());
        assert!(parse_list_reply("not json").is_empty());
        assert!(parse_list_reply(r#"{"ok":true}"#).is_empty());
    }

    #[test]
    fn preview_flattens_whitespace_and_truncates() {
        assert_eq!(preview("  hello   world  ", 80), "hello world");
        assert_eq!(preview("a\nb\tc", 80), "a b c");
        // Truncation keeps an ellipsis within the budget.
        let p = preview("abcdefghij", 5);
        assert_eq!(p, "abcd\u{2026}");
        assert_eq!(p.chars().count(), 5);
    }

    #[test]
    fn rfc3339_round_trips_a_known_instant() {
        // 2026-06-21T12:00:00Z. Cross-check against the civil-day math:
        // days_from_civil gives the day index; *86400 + 12h.
        let z = rfc3339_to_epoch("2026-06-21T12:00:00Z").unwrap();
        let plus = rfc3339_to_epoch("2026-06-21T12:00:00+00:00").unwrap();
        assert_eq!(z, plus);
        // A +02:00 stamp at the same wall-clock is 2h earlier in UTC.
        let east = rfc3339_to_epoch("2026-06-21T12:00:00+02:00").unwrap();
        assert_eq!(z - east, 7200);
        // Fractional seconds are tolerated + ignored.
        let frac = rfc3339_to_epoch("2026-06-21T12:00:00.123456Z").unwrap();
        assert_eq!(frac, z);
    }

    #[test]
    fn rfc3339_rejects_malformed() {
        assert!(rfc3339_to_epoch("nope").is_none());
        assert!(rfc3339_to_epoch("2026/06/21T12:00:00Z").is_none());
        assert!(rfc3339_to_epoch("2026-13-21T12:00:00Z").is_none()); // month 13
    }

    #[test]
    fn meta_label_drops_empty_source() {
        // `age` is already formatted by the caller; meta_label only joins it
        // to the source node.
        assert_eq!(meta_label("nodeA", "2m"), "nodeA \u{00b7} 2m");
        assert_eq!(meta_label("", "2m"), "2m");
    }
}
