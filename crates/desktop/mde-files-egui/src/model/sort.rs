//! Listing sort + hidden-file filter helpers for the [`super`] browser model.
//!
//! A behaviour-preserving relocation of the render-agnostic sort/filter cluster:
//! the hidden-file predicate the [`super::Tab`] listing applies, the human-string
//! sort-key parsers (`parse_size_bytes` / `parse_age_secs` invert FILEMGR-1's
//! `fmt_bytes` / `fmt_age` back to a monotonic ordering key), the MIME/name
//! comparators, and the in-place [`sort_rows`] driver. No egui, no state — pure
//! ordering of the value the user actually sees.

use super::*;

/// A dot-file (hidden) name. The listing display adds a trailing `/` to
/// directories, never a leading dot, so a simple prefix test is correct.
pub(super) fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The MIME sort rank for the "Type" column.
const fn mime_rank(mime: Mime) -> u8 {
    match mime {
        Mime::Folder => 0,
        Mime::Doc => 1,
        Mime::Image => 2,
        Mime::Pdf => 3,
        Mime::Archive => 4,
        Mime::Disk => 5,
    }
}

/// Parse a human size string (as FILEMGR-1's `fmt_bytes` renders it — `"512 B"`,
/// `"2 KB"`, `"5.0 MB"`, `"3.0 GB"`) back to an approximate byte count, purely as
/// a monotonic *sort key*. A folder summary (`"— · 122 items"`) or an unknown
/// shape sorts as zero — directories are grouped by dirs-first, so their order
/// falls back to the name tie-break. This is honest ordering of the value the
/// user actually sees, not a fabricated exact size.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(super) fn parse_size_bytes(s: &str) -> u64 {
    let s = s.trim();
    if s.starts_with('\u{2014}') {
        return 0; // "— · N items" — a folder summary, not a byte size.
    }
    let mut num = String::new();
    let mut unit = "";
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else {
            unit = s[i..].trim();
            break;
        }
    }
    let value: f64 = num.parse().unwrap_or(0.0);
    let mult = match unit.split_whitespace().next().unwrap_or("") {
        "GB" => 1024.0_f64.powi(3),
        "MB" => 1024.0_f64.powi(2),
        "KB" => 1024.0_f64,
        _ => 1.0,
    };
    (value * mult) as u64
}

/// Parse a human age string (as FILEMGR-1's `fmt_age` renders it — `"4 min"`,
/// `"2 h"`, `"3 d"`, `"now"`) back to an approximate "seconds since modified"
/// sort key: smaller = newer. An empty/`"—"` age sorts last (unknown). Purely
/// for ordering the value the user sees.
pub(super) fn parse_age_secs(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() || s == "\u{2014}" {
        return u64::MAX;
    }
    if s.eq_ignore_ascii_case("now") {
        return 0;
    }
    let mut it = s.split_whitespace();
    let n: u64 = it.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    let mult = match it.next().unwrap_or("") {
        "min" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "mo" => 30 * 86_400,
        "y" => 365 * 86_400,
        // "s" (seconds) and any unrecognised unit are 1 second per count.
        _ => 1,
    };
    n.saturating_mul(mult)
}

fn cmp_name(a: &FileRow, b: &FileRow) -> Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

/// Sort `rows` in place per `spec`. Directories stay grouped ahead of files when
/// `dirs_first` (independent of direction — the desktop convention); within a
/// group the chosen key orders, with name as the stable tie-break.
pub(super) fn sort_rows(rows: &mut [FileRow], spec: SortSpec) {
    rows.sort_by(|a, b| {
        if spec.dirs_first {
            match (a.is_dir(), b.is_dir()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }
        let primary = match spec.key {
            SortKey::Name => cmp_name(a, b),
            SortKey::Kind => mime_rank(a.mime)
                .cmp(&mime_rank(b.mime))
                .then_with(|| cmp_name(a, b)),
            SortKey::Size => parse_size_bytes(&a.size)
                .cmp(&parse_size_bytes(&b.size))
                .then_with(|| cmp_name(a, b)),
            SortKey::Modified => parse_age_secs(&a.age)
                .cmp(&parse_age_secs(&b.age))
                .then_with(|| cmp_name(a, b)),
        };
        match spec.dir {
            SortDir::Asc => primary,
            SortDir::Desc => primary.reverse(),
        }
    });
}
