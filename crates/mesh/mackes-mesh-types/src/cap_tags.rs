//! PLANES-3 (W82–W85) — capability tags.
//!
//! Orthogonal to the §5 role (Lighthouse ⊂ Server ⊂ Workstation):
//! a node carries zero or more **gating** capability tags that any
//! enrolled operator surface may set (W83) and that decide what duty
//! the node accepts (W84 — tags GATE, they don't merely prefer):
//!
//! - `hop`       — eligible to load relay / subnet-advertise / exit
//!   config (the Network plane).
//! - `execution` — eligible to accept job bundles beyond
//!   self-targeted ones (the Controller plane).
//! - `headless`  — GUI app-surface units are disabled here; the
//!   agent runs fully.
//!
//! Stored per-target on the replicated volume at
//! `<root>/node-tags/<hostname>.json` (any node writes any target's
//! file; the target reads its own to gate). v1 vocabulary is exactly
//! these three (builder/mirror deferred — W82).

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The v1 capability-tag vocabulary (W82).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTag {
    Hop,
    Execution,
    Headless,
}

impl CapabilityTag {
    /// Stable wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityTag::Hop => "hop",
            CapabilityTag::Execution => "execution",
            CapabilityTag::Headless => "headless",
        }
    }

    /// Parse a tag token; `None` for anything outside the v1 set
    /// (an unknown tag is refused, not silently accepted — W82).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hop" => Some(CapabilityTag::Hop),
            "execution" => Some(CapabilityTag::Execution),
            "headless" => Some(CapabilityTag::Headless),
            _ => None,
        }
    }
}

/// The directory holding every node's tag file.
#[must_use]
pub fn tags_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("node-tags")
}

/// A node's tag set (the JSON file shape).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTags {
    #[serde(default)]
    pub tags: BTreeSet<CapabilityTag>,
}

impl NodeTags {
    /// Does this set carry `tag`? The gate query (W84).
    #[must_use]
    pub fn has(&self, tag: CapabilityTag) -> bool {
        self.tags.contains(&tag)
    }
}

/// Read a node's tags (empty when unset / unparseable — an absent
/// file means "no extra capabilities", never a panic).
#[must_use]
pub fn read_tags(workgroup_root: &Path, hostname: &str) -> NodeTags {
    let path = tags_dir(workgroup_root).join(format!("{hostname}.json"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Write a node's tags (atomic temp + rename). Any enrolled surface
/// may call this for any target (W83 — mesh access is the
/// authorization; the caller audit-logs).
///
/// # Errors
/// IO / serialization failures.
pub fn write_tags(workgroup_root: &Path, hostname: &str, tags: &NodeTags) -> io::Result<PathBuf> {
    let dir = tags_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let tmp = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(tags)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_round_trips_and_refuses_unknowns() {
        for t in [
            CapabilityTag::Hop,
            CapabilityTag::Execution,
            CapabilityTag::Headless,
        ] {
            assert_eq!(CapabilityTag::parse(t.as_str()), Some(t));
        }
        assert_eq!(CapabilityTag::parse("builder"), None, "deferred — not v1");
        assert_eq!(CapabilityTag::parse(""), None);
    }

    #[test]
    fn tags_round_trip_and_gate_query() {
        let tmp = tempfile::tempdir().unwrap();
        let mut set = NodeTags::default();
        set.tags.insert(CapabilityTag::Execution);
        set.tags.insert(CapabilityTag::Hop);
        write_tags(tmp.path(), "oak", &set).unwrap();
        let back = read_tags(tmp.path(), "oak");
        assert!(back.has(CapabilityTag::Execution));
        assert!(back.has(CapabilityTag::Hop));
        assert!(!back.has(CapabilityTag::Headless));
    }

    #[test]
    fn an_untagged_node_has_no_capabilities() {
        let tmp = tempfile::tempdir().unwrap();
        let t = read_tags(tmp.path(), "ghost");
        assert!(!t.has(CapabilityTag::Execution));
        assert!(t.tags.is_empty());
    }
}
