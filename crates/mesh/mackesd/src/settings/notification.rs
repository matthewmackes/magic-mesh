//! Notification applier — v2.0.0 Phase C.5.
//!
//! The notifications server worker (Phase B.10) reads its DND state
//! from a simple flag file at
//! `$XDG_CACHE_HOME/mde/notifications-dnd` (presence → DND on,
//! absence → DND off). This applier writes / removes that file
//! when the operator flips the toggle. Location + default-expire
//! preferences live in a JSON sidecar at
//! `$XDG_CACHE_HOME/mde/notifications-prefs.json`.
//!
//! Pure-helper functions (`dnd_flag_path`, `prefs_path`,
//! `parse_dnd_state`, `parse_prefs_json`) live alongside the impl
//! so the file format is unit-testable without disk I/O.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON-shaped sidecar for non-DND notification prefs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPrefs {
    /// Corner the panel renders notifications in. Empty when unset.
    #[serde(default)]
    pub location: String,
    /// Default expire-after milliseconds. -1 = follow spec default.
    #[serde(default = "default_expire_ms")]
    pub default_expire_ms: i64,
}

const fn default_expire_ms() -> i64 {
    -1
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

/// Path of the DND flag file.
#[must_use]
pub fn dnd_flag_path() -> PathBuf {
    cache_root().join("mde").join("notifications-dnd")
}

/// Path of the notification prefs JSON sidecar.
#[must_use]
pub fn prefs_path() -> PathBuf {
    cache_root().join("mde").join("notifications-prefs.json")
}

/// Pure helper: presence of the flag file equals DND on.
#[must_use]
pub fn parse_dnd_state(path: &std::path::Path) -> bool {
    path.exists()
}

/// Pure helper: parse the prefs JSON. Returns the default when the
/// file is missing or malformed.
#[must_use]
pub fn parse_prefs_json(text: &str) -> NotificationPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Apply a `notification.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a notification key, the
/// value doesn't match the expected type, or a filesystem write
/// fails.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    match key {
        SettingKey::NotificationDoNotDisturb => {
            let on: bool = value.to_serde()?;
            let path = dnd_flag_path();
            if on {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("notification: mkdir {} failed: {e}", parent.display())
                    })?;
                }
                std::fs::write(&path, "").map_err(|e| {
                    anyhow::anyhow!("notification: write {} failed: {e}", path.display())
                })?;
            } else if path.exists() {
                std::fs::remove_file(&path).map_err(|e| {
                    anyhow::anyhow!("notification: unlink {} failed: {e}", path.display())
                })?;
            }
            Ok(())
        }
        SettingKey::NotificationLocation => {
            let location: String = value.to_serde()?;
            update_prefs(move |p| p.location = location.clone())
        }
        SettingKey::NotificationDefaultExpireMs => {
            let ms: i64 = value.to_serde()?;
            update_prefs(move |p| p.default_expire_ms = ms)
        }
        _ => anyhow::bail!("notification: {key} is not a notification key"),
    }
}

fn update_prefs(mut mutator: impl FnMut(&mut NotificationPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = if let Ok(text) = std::fs::read_to_string(&path) {
        parse_prefs_json(&text)
    } else {
        NotificationPrefs::default()
    };
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("notification: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("notification: serialize failed: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("notification: write {} failed: {e}", path.display()))?;
    Ok(())
}

/// Read the current `notification.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a notification key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    match key {
        SettingKey::NotificationDoNotDisturb => {
            SettingValue::from_serde(&parse_dnd_state(&dnd_flag_path()))
        }
        SettingKey::NotificationLocation => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.location)
        }
        SettingKey::NotificationDefaultExpireMs => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.default_expire_ms)
        }
        _ => anyhow::bail!("notification: {key} is not a notification key"),
    }
}

fn read_prefs() -> NotificationPrefs {
    let path = prefs_path();
    std::fs::read_to_string(&path)
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default()
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
    fn dnd_off_by_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            let v = current(SettingKey::NotificationDoNotDisturb).unwrap();
            let on: bool = v.to_serde().unwrap();
            assert!(!on);
        });
    }

    #[test]
    fn apply_dnd_on_then_off_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::NotificationDoNotDisturb,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            let on: bool = current(SettingKey::NotificationDoNotDisturb)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(on);
            apply(
                SettingKey::NotificationDoNotDisturb,
                &SettingValue::from_serde(&false).unwrap(),
            )
            .unwrap();
            let on: bool = current(SettingKey::NotificationDoNotDisturb)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(!on);
        });
    }

    #[test]
    fn apply_dnd_off_is_idempotent_when_flag_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::NotificationDoNotDisturb,
                &SettingValue::from_serde(&false).unwrap(),
            )
            .expect("dnd off when absent must not error");
        });
    }

    #[test]
    fn apply_location_then_current_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::NotificationLocation,
                &SettingValue::from_serde(&"top-right".to_string()).unwrap(),
            )
            .unwrap();
            let loc: String = current(SettingKey::NotificationLocation)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(loc, "top-right");
        });
    }

    #[test]
    fn apply_default_expire_then_current_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::NotificationDefaultExpireMs,
                &SettingValue::from_serde(&5000_i64).unwrap(),
            )
            .unwrap();
            let ms: i64 = current(SettingKey::NotificationDefaultExpireMs)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(ms, 5000);
        });
    }

    #[test]
    fn apply_location_preserves_default_expire() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::NotificationDefaultExpireMs,
                &SettingValue::from_serde(&7000_i64).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::NotificationLocation,
                &SettingValue::from_serde(&"bottom-left".to_string()).unwrap(),
            )
            .unwrap();
            let ms: i64 = current(SettingKey::NotificationDefaultExpireMs)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(ms, 7000);
        });
    }

    #[test]
    fn parse_prefs_json_handles_malformed() {
        let p = parse_prefs_json("not json");
        assert_eq!(p, NotificationPrefs::default());
    }

    #[test]
    fn parse_dnd_state_reads_presence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("flag");
        assert!(!parse_dnd_state(&path));
        std::fs::write(&path, "").unwrap();
        assert!(parse_dnd_state(&path));
    }

    #[test]
    fn apply_rejects_non_notification_key() {
        let v = SettingValue::from_serde(&true).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }
}
