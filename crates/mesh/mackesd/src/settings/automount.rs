//! Automount applier — v2.0.0 Phase C.6.
//!
//! Three booleans persist to `$XDG_CACHE_HOME/mde/automount.json`
//! and are honored by the udisks2-aware Workbench Devices/Removable
//! panel + the file-manager's xdg-open hook. Same sidecar pattern as
//! [`super::power`] + [`super::notification`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON sidecar shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomountPrefs {
    /// Auto-mount removable media on insert.
    #[serde(default)]
    pub on_insert: bool,
    /// Open file manager on mount.
    #[serde(default)]
    pub open_on_mount: bool,
    /// Honor autorun.sh / autorun.inf.
    #[serde(default)]
    pub autorun: bool,
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
    cache_root().join("mde").join("automount.json")
}

/// Parse the sidecar. Returns default on malformed input.
#[must_use]
pub fn parse_prefs_json(text: &str) -> AutomountPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Apply an `automount.*` setting.
///
/// # Errors
/// Returns an error when the key isn't an automount key, the value
/// isn't a bool, or the filesystem write fails.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    let on: bool = value.to_serde()?;
    match key {
        SettingKey::AutomountOnInsert => update_prefs(move |p| p.on_insert = on),
        SettingKey::AutomountOpenOnMount => update_prefs(move |p| p.open_on_mount = on),
        SettingKey::AutomountAutorun => update_prefs(move |p| p.autorun = on),
        _ => anyhow::bail!("automount: {key} is not an automount key"),
    }
}

fn update_prefs(mut mutator: impl FnMut(&mut AutomountPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = std::fs::read_to_string(&path)
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("automount: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("automount: serialize failed: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("automount: write {} failed: {e}", path.display()))
}

/// Read the current `automount.*` setting.
///
/// # Errors
/// Returns an error when the key isn't an automount key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let prefs = std::fs::read_to_string(prefs_path())
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    match key {
        SettingKey::AutomountOnInsert => SettingValue::from_serde(&prefs.on_insert),
        SettingKey::AutomountOpenOnMount => SettingValue::from_serde(&prefs.open_on_mount),
        SettingKey::AutomountAutorun => SettingValue::from_serde(&prefs.autorun),
        _ => anyhow::bail!("automount: {key} is not an automount key"),
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
    fn defaults_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            let v: bool = current(SettingKey::AutomountOnInsert)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(!v);
            let v: bool = current(SettingKey::AutomountAutorun)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(!v, "autorun defaults to false for safety");
        });
    }

    #[test]
    fn apply_round_trip_all_three_keys() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::AutomountOnInsert,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::AutomountOpenOnMount,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::AutomountAutorun,
                &SettingValue::from_serde(&false).unwrap(),
            )
            .unwrap();
            let a: bool = current(SettingKey::AutomountOnInsert)
                .unwrap()
                .to_serde()
                .unwrap();
            let b: bool = current(SettingKey::AutomountOpenOnMount)
                .unwrap()
                .to_serde()
                .unwrap();
            let c: bool = current(SettingKey::AutomountAutorun)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(a && b && !c);
        });
    }

    #[test]
    fn apply_one_key_preserves_others() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::AutomountOnInsert,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::AutomountAutorun,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            let a: bool = current(SettingKey::AutomountOnInsert)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(a, "earlier on_insert=true must survive autorun apply");
        });
    }

    #[test]
    fn parse_prefs_json_handles_malformed() {
        assert_eq!(parse_prefs_json("not json"), AutomountPrefs::default());
    }

    #[test]
    fn apply_rejects_non_automount_key() {
        let v = SettingValue::from_serde(&true).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }
}
