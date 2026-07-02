//! Chromium `Bookmarks` JSON importer (lock Q12: roots → named subfolders).
//!
//! Chromium/Chrome/Edge/Brave write a plain JSON `Bookmarks` file with a `roots`
//! object holding `bookmark_bar` / `other` / `synced`, each a folder node. A
//! node is `{ "type": "url"|"folder", "name", "url"?, "date_added", "children"? }`.
//! `date_added` is a string of microseconds since 1601-01-01 (`WebKit` epoch). No
//! logins/cookies/history are anywhere in this file.

use serde_json::Value;

use super::parsed::{ParsedBookmark, ParsedNode, ParsedTree};
use super::ImportError;
use crate::Source;

/// Microseconds between the `WebKit` epoch (1601-01-01) and the Unix epoch.
const WEBKIT_EPOCH_OFFSET_MICROS: i128 = 11_644_473_600_000_000;

/// The friendly `Imported/<Browser>/<Root>` subfolder name for a root key.
fn root_label(key: &str, name: &str) -> String {
    match key {
        "bookmark_bar" => "Bookmarks Bar".to_string(),
        "other" => "Other".to_string(),
        "synced" => "Mobile".to_string(),
        _ if !name.is_empty() => name.to_string(),
        _ => key.to_string(),
    }
}

/// Convert a Chromium `date_added` (`WebKit` microseconds, as a string) to Unix ms.
fn webkit_to_unix_ms(raw: &str) -> u64 {
    raw.trim()
        .parse::<i128>()
        .ok()
        .map(|micros| micros - WEBKIT_EPOCH_OFFSET_MICROS)
        .filter(|unix_micros| *unix_micros > 0)
        .and_then(|unix_micros| u64::try_from(unix_micros / 1000).ok())
        .unwrap_or(0)
}

/// Parse a Chromium `Bookmarks` JSON file into a [`ParsedTree`].
pub fn parse(bytes: &[u8]) -> Result<ParsedTree, ImportError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let roots_obj = value
        .get("roots")
        .and_then(Value::as_object)
        .ok_or_else(|| ImportError::Malformed("chromium bookmarks: missing 'roots'".to_string()))?;

    let mut roots = Vec::new();
    // The three well-known roots first, in a stable order (lock Q12)...
    for key in ["bookmark_bar", "other", "synced"] {
        if let Some(node) = roots_obj.get(key) {
            roots.push(parse_root(key, node));
        }
    }
    // ...then any custom/extra roots, deterministically (serde_json map order).
    for (key, node) in roots_obj {
        if matches!(key.as_str(), "bookmark_bar" | "other" | "synced") {
            continue;
        }
        if node.is_object() {
            roots.push(parse_root(key, node));
        }
    }

    Ok(ParsedTree {
        source: Source::Chromium,
        roots,
    })
}

/// Turn a root object into a named folder node.
fn parse_root(key: &str, node: &Value) -> ParsedNode {
    let name = node.get("name").and_then(Value::as_str).unwrap_or("");
    ParsedNode::Folder {
        name: root_label(key, name),
        children: parse_children(node),
    }
}

/// Parse a node's `children` array (empty when absent).
fn parse_children(node: &Value) -> Vec<ParsedNode> {
    node.get("children")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_node).collect())
        .unwrap_or_default()
}

/// Parse one tree node (`url` or `folder`); anything else is skipped.
fn parse_node(node: &Value) -> Option<ParsedNode> {
    match node.get("type").and_then(Value::as_str) {
        Some("folder") => Some(ParsedNode::Folder {
            name: node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            children: parse_children(node),
        }),
        Some("url") => {
            let url = node.get("url").and_then(Value::as_str)?.to_string();
            Some(ParsedNode::Bookmark(ParsedBookmark {
                url,
                title: node
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                tags: Vec::new(),
                added_ms: node
                    .get("date_added")
                    .and_then(Value::as_str)
                    .map_or(0, webkit_to_unix_ms),
            }))
        }
        _ => None,
    }
}
