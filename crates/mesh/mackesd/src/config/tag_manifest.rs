//! HYP-8.5 — per-tag manifest schema + loader.
//!
//! A tag manifest is a single TOML file under
//! `~/.config/mde/tags/<name>.toml` that carries the per-tag
//! compositor + UX policy that every tag-consumer (HYP-9 /
//! HYP-10 / HYP-11 / HYP-12 / HYP-14 / HYP-22, plus the
//! Portal-* tag-aware features) reads at startup. The manifest
//! is the single source of truth — no parallel `tag-colors.toml`
//! file, no scattered per-feature config.
//!
//! ## Fields
//!
//! - `name` — display name (defaults to the file stem; explicit
//!   `name = "..."` lets operators decouple file path from
//!   display).
//! - `output` — Wayland output assignment (Hyprland desc:* or
//!   sway name). Empty/missing → any output.
//! - `apps` — list of app_ids that belong to this tag.
//! - `layout` — preferred container layout (`mde` / `splith` /
//!   `splitv` / `tabbed` / `stacked`). Defaults to `mde` (the
//!   compositor's own algorithm for v6.5).
//! - `marks_default` — comma-delimited list of marks the auto-
//!   mark daemon should apply to windows joining this tag.
//! - `border_color` — hex color (CSS form, e.g. `#5b6af5`) for
//!   the per-tag border per HYP-22.
//! - `autostart` — when true, mded's tag autostart worker spawns
//!   each `apps[]` entry on first login if not already running.
//!
//! ## File path convention
//!
//! `~/.config/mde/tags/<name>.toml` (operator-edited). The
//! defaults shipped under `/usr/share/mde/tag-manifests/` are
//! copied to the user's `~/.config/mde/tags/` on first login by
//! the birthright wizard (HYP-8.5.birthright follow-on). All
//! `~/.config/` is GFS-replicated per [[project_v5_0_0_gluster_mesh]]
//! so peers see the same tag set.
//!
//! ## Fail-open contract
//!
//! Malformed TOML or unknown fields log a warning + skip the
//! manifest. The loader never returns Err on per-file parse
//! failure — only on the directory itself being unreadable for
//! reasons beyond per-file content. This keeps mded boot resilient
//! against operator typos: bad manifest → that tag missing, not
//! the whole daemon crashing.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One tag manifest. All fields except the implicit `name` are
/// optional; serde defaults cover absent ones so partial manifests
/// parse cleanly + use safe defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagManifest {
    /// Display name. When parsing from a file, this defaults to
    /// the file stem; the optional `name = "..."` in the TOML
    /// body overrides. Loader populates from filename when
    /// missing.
    #[serde(default)]
    pub name: String,
    /// Wayland output assignment. `Some("HDMI-A-1")` /
    /// `Some("desc:Dell U2715H ...")` / `None` = any output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// App IDs belonging to this tag. Used by auto-mark + tag-
    /// driven workspace router. Empty = no automatic membership.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Layout preference. `"mde"` (default) / `"splith"` /
    /// `"splitv"` / `"tabbed"` / `"stacked"`.
    #[serde(default = "default_layout")]
    pub layout: String,
    /// Comma-delimited default marks the auto-mark daemon
    /// applies. Empty = no defaults.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub marks_default: String,
    /// Per-tag border color (CSS hex, e.g. `#5b6af5`). HYP-22
    /// reads this for the focused-window border tint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    /// When true, mded's autostart worker spawns each `apps[]`
    /// entry on first login if not already running.
    #[serde(default)]
    pub autostart: bool,
}

fn default_layout() -> String {
    "mde".to_string()
}

impl Default for TagManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            output: None,
            apps: Vec::new(),
            layout: default_layout(),
            marks_default: String::new(),
            border_color: None,
            autostart: false,
        }
    }
}

/// Error surface for `load_all`. Per the fail-open contract,
/// per-file parse failures don't produce these — only directory-
/// level I/O errors do.
#[derive(Debug)]
pub enum LoadError {
    /// Directory I/O failure (missing, unreadable, etc.). The
    /// inner `io::Error` carries the specific reason.
    Io(std::io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "tag-manifest dir I/O: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Resolve the default tag-manifest directory:
/// `<XDG_CONFIG_HOME>/mde/tags/`. Falls back to `$HOME/.config`
/// when XDG_CONFIG_HOME is unset. Returns None when neither is
/// available (vanishingly rare; mded refuses to start in that
/// state anyway).
#[must_use]
pub fn default_manifests_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("mde").join("tags"))
}

/// Resolve the system-default tag-manifest directory:
/// `/usr/share/mde/tag-manifests/`. The birthright step copies
/// from here to the operator's home dir on first login.
#[must_use]
pub fn system_manifests_dir() -> PathBuf {
    PathBuf::from("/usr/share/mde/tag-manifests")
}

/// Load every `*.toml` file in `dir` as a TagManifest. Missing
/// dir returns an empty Vec (first-boot path before the
/// birthright step copies the seeds). Per the fail-open contract,
/// individual files that fail to parse are logged + skipped; the
/// loader returns Ok with the partial set.
///
/// Manifest names default to the file stem when the TOML body
/// doesn't carry an explicit `name = "..."`.
pub fn load_all(dir: &Path) -> Result<Vec<TagManifest>, LoadError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(LoadError::Io)?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "toml" {
            continue;
        }
        match parse_file(&path) {
            Ok(manifest) => out.push(manifest),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "tag_manifest: skipping malformed manifest",
                );
            }
        }
    }
    // Sort by name for deterministic ordering — operators rely on
    // a stable order when listing manifests + the Bus publish
    // sequence becomes deterministic too.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Parse a single manifest file. Public for tests; mded's
/// startup uses `load_all`.
pub fn parse_file(path: &Path) -> Result<TagManifest, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let mut manifest: TagManifest = toml::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
    // Default name to file stem when the TOML didn't set one.
    if manifest.name.is_empty() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            manifest.name = stem.to_string();
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_manifest(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{name}.toml"));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parse_canonical_voip_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            "voip",
            r##"
name = "voip"
apps = ["org.mde.voice.hud", "org.mde.voice.dial"]
layout = "splith"
border_color = "#5b6af5"
autostart = true
"##,
        );
        let m = parse_file(&path).unwrap();
        assert_eq!(m.name, "voip");
        assert_eq!(m.apps.len(), 2);
        assert_eq!(m.layout, "splith");
        assert_eq!(m.border_color.as_deref(), Some("#5b6af5"));
        assert!(m.autostart);
    }

    #[test]
    fn parse_minimal_manifest_uses_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(tmp.path(), "minimal", "");
        let m = parse_file(&path).unwrap();
        assert_eq!(m.name, "minimal"); // From file stem.
        assert_eq!(m.layout, "mde");
        assert!(m.apps.is_empty());
        assert!(!m.autostart);
        assert!(m.output.is_none());
        assert!(m.border_color.is_none());
        assert!(m.marks_default.is_empty());
    }

    #[test]
    fn parse_explicit_name_overrides_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(tmp.path(), "stem-name", r#"name = "Display Name""#);
        let m = parse_file(&path).unwrap();
        // Explicit name wins over file stem.
        assert_eq!(m.name, "Display Name");
    }

    #[test]
    fn load_missing_dir_returns_empty() {
        let path = PathBuf::from("/nonexistent/path/tags");
        let r = load_all(&path).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn load_all_picks_up_every_toml() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", r#"apps = ["one"]"#);
        write_manifest(tmp.path(), "dev", r#"apps = ["two"]"#);
        write_manifest(tmp.path(), "media", r#"apps = ["three"]"#);
        let r = load_all(tmp.path()).unwrap();
        assert_eq!(r.len(), 3);
        // Sorted by name.
        assert_eq!(r[0].name, "dev");
        assert_eq!(r[1].name, "media");
        assert_eq!(r[2].name, "voip");
    }

    #[test]
    fn load_all_skips_non_toml_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "good", "");
        // Drop a non-TOML file in the same dir.
        std::fs::write(tmp.path().join("readme.md"), "ignore me").unwrap();
        std::fs::write(tmp.path().join("noext"), "no extension").unwrap();
        let r = load_all(tmp.path()).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "good");
    }

    #[test]
    fn load_all_fail_open_on_malformed_toml() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "good", r#"apps = ["one"]"#);
        // Deliberately broken TOML — invalid syntax.
        std::fs::write(tmp.path().join("broken.toml"), "apps = not a value =\n").unwrap();
        // The loader should still return Ok with just the
        // parseable manifest, skipping `broken.toml`.
        let r = load_all(tmp.path()).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "good");
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let m = TagManifest {
            name: "round-trip".to_string(),
            output: Some("HDMI-A-1".to_string()),
            apps: vec!["a".to_string(), "b".to_string()],
            layout: "tabbed".to_string(),
            marks_default: "x,y,z".to_string(),
            border_color: Some("#42be65".to_string()),
            autostart: true,
        };
        let body = toml::to_string(&m).unwrap();
        let parsed: TagManifest = toml::from_str(&body).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn default_layout_matches_lock() {
        assert_eq!(default_layout(), "mde");
        assert_eq!(TagManifest::default().layout, "mde");
    }

    #[test]
    fn system_dir_resolves_to_usr_share() {
        let p = system_manifests_dir();
        assert_eq!(p, PathBuf::from("/usr/share/mde/tag-manifests"));
    }
}
