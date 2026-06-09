//! Wallpaper applier — v2.0.0 Phase C.7.
//!
//! Persists wallpaper path + mode to a JSON sidecar at
//! `$XDG_CACHE_HOME/mde/wallpaper.json`. The bg applet
//! (Phase E.2 / E1.2) watches this file via cosmic-config and
//! reapplies on change.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON sidecar shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperPrefs {
    /// Path to the wallpaper image.
    #[serde(default)]
    pub path: String,
    /// Render mode: stretch / fit / fill / center / tile.
    #[serde(default)]
    pub mode: String,
}

fn cache_root() -> PathBuf {
    if let Ok(s) = std::env::var("XDG_CACHE_HOME") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cache");
    }
    PathBuf::from("/tmp")
}

/// Sidecar path.
#[must_use]
pub fn prefs_path() -> PathBuf {
    cache_root().join("mde").join("wallpaper.json")
}

/// Pure helper: parse the sidecar. Default on malformed.
#[must_use]
pub fn parse_prefs_json(text: &str) -> WallpaperPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Valid render modes.
pub const VALID_MODES: &[&str] = &["stretch", "fit", "fill", "center", "tile"];

/// Pure helper: is `mode` valid? Empty string accepted (treated as
/// "unset, applet picks default").
#[must_use]
pub fn is_valid_mode(mode: &str) -> bool {
    mode.is_empty() || VALID_MODES.contains(&mode)
}

/// Apply a `wallpaper.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a wallpaper key, the value
/// fails to deserialize, the mode isn't valid, or the write fails.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    match key {
        SettingKey::WallpaperPath => {
            let path: String = value.to_serde()?;
            update_prefs(move |p| p.path = path.clone())
        }
        SettingKey::WallpaperMode => {
            let mode: String = value.to_serde()?;
            if !is_valid_mode(&mode) {
                anyhow::bail!("wallpaper: {mode} not in {:?}", VALID_MODES);
            }
            update_prefs(move |p| p.mode = mode.clone())
        }
        _ => anyhow::bail!("wallpaper: {key} is not a wallpaper key"),
    }
}

fn update_prefs(mut mutator: impl FnMut(&mut WallpaperPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = std::fs::read_to_string(&path)
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("wallpaper: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("wallpaper: serialize: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("wallpaper: write {} failed: {e}", path.display()))
}

/// Read the current `wallpaper.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a wallpaper key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let prefs = std::fs::read_to_string(prefs_path())
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    match key {
        SettingKey::WallpaperPath => SettingValue::from_serde(&prefs.path),
        SettingKey::WallpaperMode => SettingValue::from_serde(&prefs.mode),
        _ => anyhow::bail!("wallpaper: {key} is not a wallpaper key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn with_xdg<R>(tmp: &std::path::Path, body: impl FnOnce() -> R) -> R {
        let lock = ENV_LOCK.get_or_init(|| Mutex::new(()));
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", tmp);
        let r = body();
        match prev {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        r
    }

    #[test]
    fn is_valid_mode_accepts_known_values_and_empty() {
        assert!(is_valid_mode(""));
        for m in VALID_MODES {
            assert!(is_valid_mode(m));
        }
    }

    #[test]
    fn is_valid_mode_rejects_unknown() {
        assert!(!is_valid_mode("bogus"));
        assert!(!is_valid_mode("Fill"));
    }

    #[test]
    fn apply_path_then_current_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::WallpaperPath,
                &SettingValue::from_serde(&"/usr/share/wp/sun.jpg".to_string()).unwrap(),
            )
            .unwrap();
            let p: String = current(SettingKey::WallpaperPath)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(p, "/usr/share/wp/sun.jpg");
        });
    }

    #[test]
    fn apply_mode_then_current_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::WallpaperMode,
                &SettingValue::from_serde(&"fill".to_string()).unwrap(),
            )
            .unwrap();
            let m: String = current(SettingKey::WallpaperMode)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(m, "fill");
        });
    }

    #[test]
    fn apply_rejects_invalid_mode() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            let r = apply(
                SettingKey::WallpaperMode,
                &SettingValue::from_serde(&"bogus".to_string()).unwrap(),
            );
            assert!(r.is_err());
        });
    }

    #[test]
    fn parse_prefs_json_handles_malformed() {
        assert_eq!(parse_prefs_json("nope"), WallpaperPrefs::default());
    }

    #[test]
    fn apply_rejects_non_wallpaper_key() {
        let v = SettingValue::from_serde(&"x".to_string()).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }
}
