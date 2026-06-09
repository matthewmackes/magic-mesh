//! Portal-53.b.types-share (2026-05-27) — sway window-rules data
//! layer.
//!
//! Originally lived in `crates/mackesd/src/workers/window_rules.rs`
//! alongside the `WindowRulesWorker` that applies the rules to a
//! running sway. Lifted here so `mde-portal`'s Hub right-click
//! modal (Portal-53.b) can read + write the same TOML file
//! without taking a dependency on mackesd. The worker continues
//! to consume these types via re-export.
//!
//! Store path: `<XDG_CONFIG_HOME>/mde/window-rules.toml`.
//!
//! Schema (TOML):
//!
//! ```toml
//! schema_version = 1
//!
//! [[rule]]
//! match = "Firefox"
//! floating = false
//! sticky = false
//! fullscreen_on_start = false
//! border_width = 4
//! mark = "browser"
//! assign_workspace = 2
//! ```
//!
//! Atomic write via temp + rename so the worker's mtime-poll
//! never sees a half-written file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One window rule. All fields except `r#match` are optional; an
/// empty rule (only `match` set) is a no-op but parses cleanly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRule {
    /// app_id criterion (e.g. `"Firefox"`, `"foot"`). Matched
    /// against sway's `app_id` exactly. Required.
    #[serde(rename = "match")]
    pub match_app_id: String,
    /// `floating enable` on window::new when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<bool>,
    /// `sticky enable` on window::new when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<bool>,
    /// `fullscreen enable` on window::new when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen_on_start: Option<bool>,
    /// `border normal <n>` on window::new when set. Sway's
    /// border-width takes pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_width: Option<u32>,
    /// `mark <name>` on window::new when set. Names are taxonomy-
    /// free at this layer — operators can use any string they like
    /// (Portal-48's auto-mark daemon uses its own fixed-5 taxonomy
    /// + skips windows that already carry a mark, so a rule-imposed
    /// mark wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    /// `move container to workspace number <n>` on window::new
    /// when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign_workspace: Option<i32>,
}

/// Top-level TOML file shape. `rules:` is the `Vec<WindowRule>`
/// array (TOML key `[[rule]]`); `schema_version` is informational
/// + bumps on backwards-incompatible changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRulesFile {
    /// Schema version. Defaults to 1 on load.
    #[serde(default = "schema_version_default")]
    pub schema_version: u32,
    /// All rules in load order.
    #[serde(default, rename = "rule")]
    pub rules: Vec<WindowRule>,
}

impl Default for WindowRulesFile {
    fn default() -> Self {
        Self {
            schema_version: schema_version_default(),
            rules: Vec::new(),
        }
    }
}

fn schema_version_default() -> u32 {
    1
}

/// Error surface for `WindowRulesFile` I/O. Read-side covers
/// I/O failure + TOML parse failure; write-side reuses the I/O
/// variant for filesystem errors + a TOML-serialize variant for
/// the (vanishingly rare) encode failure path.
#[derive(Debug)]
pub enum RulesError {
    /// Filesystem I/O failure on read or write.
    Io(io::Error),
    /// TOML parse failure (read path).
    Parse(toml::de::Error),
    /// TOML serialize failure (write path).
    Serialize(toml::ser::Error),
    /// Path resolution failure (`$XDG_CONFIG_HOME` + `$HOME` both
    /// unset — vanishingly rare).
    PathResolution,
}

impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "window-rules I/O: {e}"),
            Self::Parse(e) => write!(f, "window-rules parse: {e}"),
            Self::Serialize(e) => write!(f, "window-rules serialize: {e}"),
            Self::PathResolution => write!(f, "could not resolve window-rules.toml path"),
        }
    }
}

impl std::error::Error for RulesError {}

impl From<io::Error> for RulesError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<toml::de::Error> for RulesError {
    fn from(e: toml::de::Error) -> Self {
        Self::Parse(e)
    }
}

impl From<toml::ser::Error> for RulesError {
    fn from(e: toml::ser::Error) -> Self {
        Self::Serialize(e)
    }
}

impl WindowRulesFile {
    /// Load from `<XDG_CONFIG_HOME>/mde/window-rules.toml`. Missing
    /// file → empty default (first-boot path).
    pub fn load_default() -> Result<Self, RulesError> {
        let path = default_rules_path()?;
        Self::load_from(&path)
    }

    /// Load from explicit path. Missing file → empty default.
    pub fn load_from(path: &Path) -> Result<Self, RulesError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        let file: WindowRulesFile = toml::from_str(&raw)?;
        Ok(file)
    }

    /// Save to `<XDG_CONFIG_HOME>/mde/window-rules.toml`.
    pub fn save_default(&self) -> Result<(), RulesError> {
        let path = default_rules_path()?;
        self.save_to(&path)
    }

    /// Save to explicit path. Atomic via temp + rename. Creates the
    /// parent directory if missing.
    pub fn save_to(&self, path: &Path) -> Result<(), RulesError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)?;
        let mut tmp = path.to_path_buf();
        tmp.set_extension("toml.tmp");
        fs::write(&tmp, body)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Append a rule. Convenience for the Hub modal's "save new
    /// rule" path. Does not deduplicate — operators can ship two
    /// rules with the same `match` (they fire in load order).
    pub fn push_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    /// Replace the first rule whose `match_app_id` matches the
    /// given key. Returns true when a rule was replaced; false
    /// when no matching rule exists (caller can `push_rule` then
    /// for upsert semantics).
    pub fn replace_first_matching(&mut self, match_app_id: &str, rule: WindowRule) -> bool {
        for existing in self.rules.iter_mut() {
            if existing.match_app_id == match_app_id {
                *existing = rule;
                return true;
            }
        }
        false
    }

    /// Find the first rule whose `match_app_id` matches the key.
    /// Returns `None` when no such rule exists. Useful for seeding
    /// the edit modal with existing values.
    #[must_use]
    pub fn find_first_matching(&self, match_app_id: &str) -> Option<&WindowRule> {
        self.rules.iter().find(|r| r.match_app_id == match_app_id)
    }
}

/// Default path for the rules TOML:
/// `<XDG_CONFIG_HOME>/mde/window-rules.toml`.
pub fn default_rules_path() -> Result<PathBuf, RulesError> {
    let cfg = dirs::config_dir().ok_or(RulesError::PathResolution)?;
    Ok(cfg.join("mde").join("window-rules.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rule() -> WindowRule {
        WindowRule {
            match_app_id: "firefox".to_string(),
            floating: Some(true),
            sticky: None,
            fullscreen_on_start: None,
            border_width: Some(4),
            mark: Some("browser".to_string()),
            assign_workspace: Some(2),
        }
    }

    #[test]
    fn empty_file_round_trips() {
        let f = WindowRulesFile::default();
        let toml_body = toml::to_string(&f).unwrap();
        let parsed: WindowRulesFile = toml::from_str(&toml_body).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert!(parsed.rules.is_empty());
    }

    #[test]
    fn single_rule_round_trips() {
        let mut f = WindowRulesFile::default();
        f.push_rule(sample_rule());
        let body = toml::to_string(&f).unwrap();
        let parsed: WindowRulesFile = toml::from_str(&body).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].match_app_id, "firefox");
        assert_eq!(parsed.rules[0].floating, Some(true));
        assert_eq!(parsed.rules[0].border_width, Some(4));
        assert_eq!(parsed.rules[0].mark.as_deref(), Some("browser"));
        assert_eq!(parsed.rules[0].assign_workspace, Some(2));
    }

    #[test]
    fn optional_fields_skip_on_serialize() {
        let rule = WindowRule {
            match_app_id: "foot".to_string(),
            ..WindowRule::default()
        };
        let mut f = WindowRulesFile::default();
        f.push_rule(rule);
        let body = toml::to_string(&f).unwrap();
        // Only the match field should show up; every Option::None
        // is skipped per the serde attributes.
        assert!(body.contains("match = \"foot\""));
        assert!(!body.contains("floating"));
        assert!(!body.contains("sticky"));
        assert!(!body.contains("mark"));
    }

    #[test]
    fn push_rule_appends_to_end() {
        let mut f = WindowRulesFile::default();
        f.push_rule(WindowRule {
            match_app_id: "a".to_string(),
            ..WindowRule::default()
        });
        f.push_rule(WindowRule {
            match_app_id: "b".to_string(),
            ..WindowRule::default()
        });
        assert_eq!(f.rules.len(), 2);
        assert_eq!(f.rules[0].match_app_id, "a");
        assert_eq!(f.rules[1].match_app_id, "b");
    }

    #[test]
    fn replace_first_matching_swaps_in_place() {
        let mut f = WindowRulesFile::default();
        f.push_rule(WindowRule {
            match_app_id: "firefox".to_string(),
            floating: Some(false),
            ..WindowRule::default()
        });
        let replaced = f.replace_first_matching(
            "firefox",
            WindowRule {
                match_app_id: "firefox".to_string(),
                floating: Some(true),
                ..WindowRule::default()
            },
        );
        assert!(replaced);
        assert_eq!(f.rules[0].floating, Some(true));
    }

    #[test]
    fn replace_first_matching_returns_false_on_miss() {
        let mut f = WindowRulesFile::default();
        f.push_rule(WindowRule {
            match_app_id: "firefox".to_string(),
            ..WindowRule::default()
        });
        let replaced = f.replace_first_matching("absent", WindowRule::default());
        assert!(!replaced);
        assert_eq!(f.rules.len(), 1);
        assert_eq!(f.rules[0].match_app_id, "firefox");
    }

    #[test]
    fn find_first_matching_returns_match() {
        let mut f = WindowRulesFile::default();
        f.push_rule(sample_rule());
        let found = f.find_first_matching("firefox").unwrap();
        assert_eq!(found.match_app_id, "firefox");
        assert_eq!(found.border_width, Some(4));
        assert!(f.find_first_matching("nonexistent").is_none());
    }

    #[test]
    fn load_missing_file_returns_empty_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope/window-rules.toml");
        let f = WindowRulesFile::load_from(&path).unwrap();
        assert!(f.rules.is_empty());
        assert_eq!(f.schema_version, 1);
    }

    #[test]
    fn save_and_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/window-rules.toml");
        let mut f = WindowRulesFile::default();
        f.push_rule(sample_rule());
        f.save_to(&path).unwrap();
        let loaded = WindowRulesFile::load_from(&path).unwrap();
        assert_eq!(loaded, f);
        // Atomic write doesn't leave a `.toml.tmp` sibling.
        let sibling = path.with_extension("toml.tmp");
        assert!(!sibling.exists());
    }

    #[test]
    fn pre_schema_files_load_with_default_version() {
        // A bare `[[rule]]` array without schema_version still
        // loads — serde's default fills in version 1. This keeps
        // backwards compat with first-version operator-written
        // files.
        let body = r#"
[[rule]]
match = "firefox"
"#;
        let f: WindowRulesFile = toml::from_str(body).unwrap();
        assert_eq!(f.schema_version, 1);
        assert_eq!(f.rules.len(), 1);
    }
}
