//! Universal Netscape bookmark-HTML importer (lock Q13/Q14 — Safari via export).
//!
//! The Netscape format (Firefox/Chrome/Safari/Edge all export it) is a nested
//! `<DL>` list: a `<DT><H3>Name</H3>` introduces a folder whose children are the
//! following `<DL>…</DL>`, and a `<DT><A HREF=…>Title</A>` is a bookmark, with
//! optional `ADD_DATE` (epoch seconds), `TAGS` (comma list) and `ICON` attrs.
//!
//! We hand-roll a tolerant tag scanner rather than pull an HTML engine into a
//! headless services crate: exports are not well-formed XML (unclosed `<DT>`,
//! bare `<p>`), and no HTML crate is in the airgapped lockfile. The scanner is
//! forgiving — unknown tags are skipped and unclosed folders are flushed at EOF.
//! No logins/cookies/history exist in an HTML bookmark export.

use super::parsed::{ParsedBookmark, ParsedNode, ParsedTree};
use crate::Source;

/// Parse a Netscape bookmark-HTML document into a [`ParsedTree`].
///
/// The source defaults to [`Source::NetscapeHtml`]; a caller that knows the file
/// is a Safari export overrides it via `super::import_file_as`.
pub fn parse(html: &str) -> ParsedTree {
    ParsedTree {
        source: Source::NetscapeHtml,
        roots: parse_nodes(html),
    }
}

/// One open list level while scanning.
struct Scope {
    /// `Some` for a named folder's `<DL>`, `None` for the outermost/anonymous one.
    name: Option<String>,
    children: Vec<ParsedNode>,
}

/// Scan the document into a node tree.
fn parse_nodes(html: &str) -> Vec<ParsedNode> {
    let lower = html.to_ascii_lowercase();
    // A sentinel root scope so `push`/`close` never underflow.
    let mut stack: Vec<Scope> = vec![Scope {
        name: None,
        children: Vec::new(),
    }];
    let mut pending_folder: Option<String> = None;
    let mut i = 0usize;

    while i < html.len() {
        let Some(lt) = html[i..].find('<') else { break };
        let lt = i + lt;
        let Some(gt_rel) = html[lt..].find('>') else {
            break;
        };
        let gt = lt + gt_rel;
        let inner_lower = &lower[lt + 1..gt];
        let inner_raw = &html[lt + 1..gt];

        match tag_name(inner_lower) {
            "dl" => {
                stack.push(Scope {
                    name: pending_folder.take(),
                    children: Vec::new(),
                });
                i = gt + 1;
            }
            "/dl" => {
                close_scope(&mut stack);
                i = gt + 1;
            }
            "h3" => {
                let (text, end) = element_text(html, &lower, gt + 1, "h3");
                pending_folder = Some(html_unescape(text).trim().to_string());
                i = end;
            }
            "a" => {
                let (text, end) = element_text(html, &lower, gt + 1, "a");
                if let Some(href) = find_attr(inner_raw, "href") {
                    let bookmark = ParsedBookmark {
                        url: html_unescape(&href),
                        title: html_unescape(text).trim().to_string(),
                        tags: find_attr(inner_raw, "tags")
                            .map(|t| split_tags(&t))
                            .unwrap_or_default(),
                        added_ms: attr_epoch_ms(inner_raw, "add_date"),
                    };
                    if let Some(top) = stack.last_mut() {
                        top.children.push(ParsedNode::Bookmark(bookmark));
                    }
                }
                i = end;
            }
            _ => i = gt + 1,
        }
    }

    // Flush any unclosed folders (malformed export), then return the root list.
    while stack.len() > 1 {
        close_scope(&mut stack);
    }
    stack.pop().map(|s| s.children).unwrap_or_default()
}

/// Pop the top scope into its parent: a named scope becomes a folder node, an
/// anonymous one splices its children up. No-op if only the sentinel remains.
fn close_scope(stack: &mut Vec<Scope>) {
    if stack.len() < 2 {
        return;
    }
    let Some(scope) = stack.pop() else { return };
    let Some(parent) = stack.last_mut() else {
        return;
    };
    match scope.name {
        Some(name) => parent.children.push(ParsedNode::Folder {
            name,
            children: scope.children,
        }),
        None => parent.children.extend(scope.children),
    }
}

/// The leading tag token of a `<…>` interior (e.g. `"a"`, `"/dl"`, `"h3"`).
fn tag_name(inner_lower: &str) -> &str {
    inner_lower
        .trim_start()
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
}

/// The text between `start` and the next `</tag>` (case-insensitive), plus the
/// byte offset just past that close tag (or EOF if unclosed).
fn element_text<'a>(html: &'a str, lower: &str, start: usize, tag: &str) -> (&'a str, usize) {
    let needle = format!("</{tag}");
    lower[start..].find(&needle).map_or_else(
        || (&html[start..], html.len()),
        |rel| {
            let close = start + rel;
            let end = html[close..]
                .find('>')
                .map_or(html.len(), |g| close + g + 1);
            (&html[start..close], end)
        },
    )
}

/// Find a (case-insensitive) attribute value on a tag's interior.
fn find_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let at = from + rel;
        from = at + name.len();
        let boundary = at == 0 || bytes[at - 1].is_ascii_whitespace();
        if !boundary {
            continue;
        }
        let mut j = at + name.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            continue;
        }
        let quote = bytes[j];
        if quote == b'"' || quote == b'\'' {
            let value_start = j + 1;
            if let Some(rel_end) = tag[value_start..].find(quote as char) {
                return Some(tag[value_start..value_start + rel_end].to_string());
            }
        } else {
            let end = tag[j..]
                .find(char::is_whitespace)
                .map_or(tag.len(), |p| j + p);
            return Some(tag[j..end].to_string());
        }
    }
    None
}

/// Read an epoch-seconds attribute and return it in milliseconds (or 0).
fn attr_epoch_ms(tag: &str, name: &str) -> u64 {
    find_attr(tag, name)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .and_then(|v| u64::try_from(v).ok())
        .map_or(0, |secs| secs.saturating_mul(1000))
}

/// Split a `TAGS="a, b,c"` value into trimmed, non-empty tags.
fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// Decode the small set of HTML entities that appear in bookmark exports, plus
/// numeric (`&#NN;` / `&#xNN;`) references.
fn html_unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        if bytes[i] == b'&' {
            if let Some(semi_rel) = text[i..].find(';') {
                if let Some(ch) = decode_entity(&text[i + 1..i + semi_rel]) {
                    out.push(ch);
                    i += semi_rel + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Decode one entity body (the text between `&` and `;`).
fn decode_entity(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        _ => decode_numeric(body),
    }
}

/// Decode a numeric entity body (`#NN` decimal or `#xNN`/`#XNN` hex).
fn decode_numeric(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = digits.strip_prefix(['x', 'X']).map_or_else(
        || digits.parse::<u32>().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )?;
    char::from_u32(code)
}
