//! Portal-50.b (v6.0, R12-Q11 — per-workspace override half) —
//! per-workspace layout overrides.
//!
//! When the operator clicks the ✕ button on Portal-50's prompt-on-
//! change layout banner, they're saying "this layout is good for
//! this workspace, but don't change the tag default." This module
//! ships the data layer that records that decision per workspace:
//!
//!   * Store path: `<XDG_DATA_HOME>/mde/workspaces.json`.
//!   * Schema: `{ "<workspace_num>": { "layout_override": "<layout>" }, ... }`.
//!   * Consumers: `tag_layout` worker (Portal-44) reads the override
//!     on `window::new` events and applies it instead of the tag's
//!     `default_layout` when set. The override always wins.
//!   * Atomic write via temp + rename.
//!
//! The schema is intentionally JSON-object-keyed-on-string rather
//! than a Vec so per-workspace lookups are O(log n) without
//! re-deriving an index. JSON object keys are strings; Rust
//! `BTreeMap<String, _>` preserves the key set in stable order.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One workspace's overrides. Currently just `layout_override`; the
/// shape leaves room for future per-workspace fields (output
/// override, tag override, etc.) without a schema_version bump.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceOverride {
    /// Layout name (`splith` / `splitv` / `tabbed` / `stacked`).
    /// `None` = no override; the owning tag's default_layout wins
    /// (or sway's natural layout for tagless workspaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_override: Option<String>,
}

/// Top-level overrides file. Wraps `BTreeMap<String, WorkspaceOverride>`
/// with a schema_version + atomic-write helpers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceOverridesFile {
    /// Schema version. Defaults to 1 on load.
    #[serde(default = "schema_version_default")]
    pub schema_version: u32,
    /// Map of workspace_num (as string) → per-workspace overrides.
    /// String keys because JSON object keys are strings; the helpers
    /// below convert i32 ↔ String at the API boundary.
    #[serde(default)]
    pub overrides: BTreeMap<String, WorkspaceOverride>,
}

impl Default for WorkspaceOverridesFile {
    fn default() -> Self {
        Self {
            schema_version: schema_version_default(),
            overrides: BTreeMap::new(),
        }
    }
}

fn schema_version_default() -> u32 {
    1
}

/// Error surface for `WorkspaceOverridesFile` I/O.
#[derive(Debug)]
pub enum OverridesError {
    /// I/O failure on read or write.
    Io(io::Error),
    /// JSON parse failure.
    Parse(serde_json::Error),
    /// Path resolution failure (`$XDG_DATA_HOME` + `$HOME` both
    /// unset — vanishingly rare).
    PathResolution,
}

impl std::fmt::Display for OverridesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "workspace-overrides I/O: {e}"),
            Self::Parse(e) => write!(f, "workspace-overrides parse: {e}"),
            Self::PathResolution => write!(f, "could not resolve workspaces.json path"),
        }
    }
}

impl std::error::Error for OverridesError {}

impl From<io::Error> for OverridesError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for OverridesError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e)
    }
}

impl WorkspaceOverridesFile {
    /// Load from `<XDG_DATA_HOME>/mde/workspaces.json`. Missing
    /// file → empty default (first-boot path).
    pub fn load_default() -> Result<Self, OverridesError> {
        let path = default_overrides_path()?;
        Self::load_from(&path)
    }

    /// Load from explicit path. Missing file → empty default.
    pub fn load_from(path: &Path) -> Result<Self, OverridesError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        let file: WorkspaceOverridesFile = serde_json::from_str(&raw)?;
        Ok(file)
    }

    /// Save to `<XDG_DATA_HOME>/mde/workspaces.json`.
    pub fn save_default(&self) -> Result<(), OverridesError> {
        let path = default_overrides_path()?;
        self.save_to(&path)
    }

    /// Save to explicit path. Atomic via temp + rename. Creates the
    /// parent directory if missing.
    pub fn save_to(&self, path: &Path) -> Result<(), OverridesError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let pretty = serde_json::to_string_pretty(self)?;
        let mut tmp = path.to_path_buf();
        tmp.set_extension("json.tmp");
        fs::write(&tmp, pretty)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Read the layout override for workspace `num`, if any.
    #[must_use]
    pub fn layout_override(&self, num: i32) -> Option<&str> {
        self.overrides
            .get(&num.to_string())
            .and_then(|o| o.layout_override.as_deref())
    }

    /// Set the layout override for workspace `num`. Replaces any
    /// existing override for that workspace.
    pub fn set_layout_override(&mut self, num: i32, layout: impl Into<String>) {
        let entry = self.overrides.entry(num.to_string()).or_default();
        entry.layout_override = Some(layout.into());
    }

    /// Clear the layout override for workspace `num`. Returns true
    /// if an override was present + cleared. Removes the workspace
    /// entry entirely when it has no remaining fields set, keeping
    /// the file tidy.
    pub fn clear_layout_override(&mut self, num: i32) -> bool {
        let key = num.to_string();
        let removed = match self.overrides.get_mut(&key) {
            Some(entry) if entry.layout_override.is_some() => {
                entry.layout_override = None;
                true
            }
            _ => false,
        };
        // If the entry is now empty (no fields set), drop it from
        // the map so workspaces.json stays small.
        if let Some(entry) = self.overrides.get(&key) {
            if entry.layout_override.is_none() {
                self.overrides.remove(&key);
            }
        }
        removed
    }
}

/// Resolve `<XDG_DATA_HOME>/mde/workspaces.json`.
pub fn default_overrides_path() -> Result<PathBuf, OverridesError> {
    let data_home = dirs::data_dir().ok_or(OverridesError::PathResolution)?;
    Ok(data_home.join("mde").join("workspaces.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips() {
        let f = WorkspaceOverridesFile::default();
        let json = serde_json::to_string(&f).unwrap();
        let parsed: WorkspaceOverridesFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert!(parsed.overrides.is_empty());
    }

    #[test]
    fn set_and_read_layout_override() {
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(1, "tabbed");
        assert_eq!(f.layout_override(1), Some("tabbed"));
        // Unset workspace returns None.
        assert!(f.layout_override(2).is_none());
    }

    #[test]
    fn set_replaces_existing_override() {
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(1, "tabbed");
        f.set_layout_override(1, "splitv");
        assert_eq!(f.layout_override(1), Some("splitv"));
    }

    #[test]
    fn clear_removes_override_and_tidy_entry() {
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(1, "tabbed");
        assert!(f.clear_layout_override(1));
        assert!(f.layout_override(1).is_none());
        // After clearing the only field, the entry is dropped
        // from the map so the file stays tidy.
        assert!(f.overrides.is_empty());
        // Clearing an absent override is a no-op + returns false.
        assert!(!f.clear_layout_override(1));
        assert!(!f.clear_layout_override(99));
    }

    #[test]
    fn load_missing_file_returns_empty_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope/workspaces.json");
        let f = WorkspaceOverridesFile::load_from(&path).unwrap();
        assert!(f.overrides.is_empty());
        assert_eq!(f.schema_version, 1);
    }

    #[test]
    fn save_and_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/workspaces.json");
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(1, "tabbed");
        f.set_layout_override(2, "splitv");
        f.save_to(&path).unwrap();
        let loaded = WorkspaceOverridesFile::load_from(&path).unwrap();
        assert_eq!(loaded, f);
        // Atomic write doesn't leave a `.json.tmp` sibling.
        let sibling = path.with_extension("json.tmp");
        assert!(!sibling.exists());
    }

    #[test]
    fn json_shape_matches_design_lock() {
        // Lock the exact on-disk shape so consumers can rely on the
        // contract without re-reading this crate. Workspace nums
        // are JSON object keys (strings); layout_override sits
        // under each.
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(1, "tabbed");
        let json = serde_json::to_string(&f).unwrap();
        // Either `"schema_version":1,"overrides":...` or the other
        // field order — serde derives field order from struct
        // declaration. Lock the substring matches.
        assert!(json.contains(r#""schema_version":1"#));
        assert!(json.contains(r#""overrides""#));
        assert!(json.contains(r#""1":{"layout_override":"tabbed"}"#));
    }

    #[test]
    fn negative_workspace_nums_round_trip() {
        // Sway's internal scratchpad meta-workspaces use num = -1.
        // We don't expect overrides for those, but the schema
        // tolerates them.
        let mut f = WorkspaceOverridesFile::default();
        f.set_layout_override(-1, "tabbed");
        assert_eq!(f.layout_override(-1), Some("tabbed"));
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""-1":{"layout_override":"tabbed"}"#));
    }

    #[test]
    fn pre_schema_files_load_with_default_version() {
        let json = r#"{"overrides":{"1":{"layout_override":"tabbed"}}}"#;
        let f: WorkspaceOverridesFile = serde_json::from_str(json).unwrap();
        assert_eq!(f.schema_version, 1);
        assert_eq!(f.layout_override(1), Some("tabbed"));
    }
}
