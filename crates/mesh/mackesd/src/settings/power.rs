//! Power applier — v2.0.0 Phase C.4.
//!
//! Three concerns map across two control surfaces:
//!
//!   1. `PowerProfile` (power-saver / balanced / performance) —
//!      shells out to `powerprofilesctl set <profile>` which routes
//!      through `power-profiles-daemon` over DBus. Read path:
//!      `powerprofilesctl get`.
//!   2. `PowerLidAction` + `PowerSuspendIdleBatteryS` +
//!      `PowerSuspendIdleAcS` — persist to a JSON sidecar at
//!      `$XDG_CACHE_HOME/mde/power-prefs.json`. The session worker
//!      (mde-session, Phase D) reads the sidecar at login to install
//!      the matching logind drop-in + swayidle config; this applier
//!      owns the read-modify-write of the file.
//!   3. `PowerPresentationMode` — writes / removes a flag file at
//!      `$XDG_CACHE_HOME/mde/power-caffeine` (presence = inhibit
//!      idle/lock). The session worker watches it via inotify.
//!
//! Pure-helper functions (`prefs_path`, `caffeine_path`,
//! `parse_prefs_json`) live alongside so the file format is
//! unit-testable without disk I/O.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON sidecar shape for the keys persisted to disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerPrefs {
    /// `nothing` / `suspend` / `hibernate` / `poweroff`. Empty
    /// when unset.
    #[serde(default)]
    pub lid_action: String,
    /// Idle-suspend timeout (battery), seconds. 0 = never.
    #[serde(default)]
    pub suspend_idle_battery_s: u64,
    /// Idle-suspend timeout (AC), seconds. 0 = never.
    #[serde(default)]
    pub suspend_idle_ac_s: u64,
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

/// JSON sidecar path.
#[must_use]
pub fn prefs_path() -> PathBuf {
    cache_root().join("mde").join("power-prefs.json")
}

/// "Caffeine" inhibit flag path.
#[must_use]
pub fn caffeine_path() -> PathBuf {
    cache_root().join("mde").join("power-caffeine")
}

/// Parse a `PowerPrefs` JSON string. Returns the default on
/// malformed input.
#[must_use]
pub fn parse_prefs_json(text: &str) -> PowerPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Apply a `power.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a power key, the value
/// doesn't fit, or a filesystem / subprocess call fails.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    match key {
        SettingKey::PowerProfile => {
            let profile: String = value.to_serde()?;
            apply_profile(&profile)
        }
        SettingKey::PowerLidAction => {
            let action: String = value.to_serde()?;
            update_prefs(move |p| p.lid_action = action.clone())
        }
        SettingKey::PowerSuspendIdleBatteryS => {
            let secs: u64 = value.to_serde()?;
            update_prefs(move |p| p.suspend_idle_battery_s = secs)
        }
        SettingKey::PowerSuspendIdleAcS => {
            let secs: u64 = value.to_serde()?;
            update_prefs(move |p| p.suspend_idle_ac_s = secs)
        }
        SettingKey::PowerPresentationMode => {
            let on: bool = value.to_serde()?;
            apply_caffeine(on)
        }
        _ => anyhow::bail!("power: {key} is not a power key"),
    }
}

fn apply_profile(profile: &str) -> anyhow::Result<()> {
    let out = std::process::Command::new("powerprofilesctl")
        .args(["set", profile])
        .output()
        .map_err(|e| anyhow::anyhow!("power: powerprofilesctl spawn failed: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "power: powerprofilesctl set {profile} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn apply_caffeine(on: bool) -> anyhow::Result<()> {
    let path = caffeine_path();
    if on {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("power: mkdir {} failed: {e}", parent.display()))?;
        }
        std::fs::write(&path, "")
            .map_err(|e| anyhow::anyhow!("power: write {} failed: {e}", path.display()))?;
    } else if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("power: unlink {} failed: {e}", path.display()))?;
    }
    Ok(())
}

fn update_prefs(mut mutator: impl FnMut(&mut PowerPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = if let Ok(text) = std::fs::read_to_string(&path) {
        parse_prefs_json(&text)
    } else {
        PowerPrefs::default()
    };
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("power: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("power: serialize failed: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("power: write {} failed: {e}", path.display()))?;
    Ok(())
}

/// Read the current `power.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a power key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    match key {
        SettingKey::PowerProfile => {
            let out = std::process::Command::new("powerprofilesctl")
                .arg("get")
                .output()
                .map_err(|e| anyhow::anyhow!("power: powerprofilesctl get failed: {e}"))?;
            if !out.status.success() {
                anyhow::bail!(
                    "power: powerprofilesctl get failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            SettingValue::from_serde(&raw)
        }
        SettingKey::PowerLidAction => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.lid_action)
        }
        SettingKey::PowerSuspendIdleBatteryS => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.suspend_idle_battery_s)
        }
        SettingKey::PowerSuspendIdleAcS => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.suspend_idle_ac_s)
        }
        SettingKey::PowerPresentationMode => SettingValue::from_serde(&caffeine_path().exists()),
        _ => anyhow::bail!("power: {key} is not a power key"),
    }
}

fn read_prefs() -> PowerPrefs {
    std::fs::read_to_string(prefs_path())
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
    fn parse_prefs_json_handles_malformed() {
        let p = parse_prefs_json("not json");
        assert_eq!(p, PowerPrefs::default());
    }

    #[test]
    fn parse_prefs_json_round_trips_through_serde() {
        let p = PowerPrefs {
            lid_action: "suspend".into(),
            suspend_idle_battery_s: 600,
            suspend_idle_ac_s: 0,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(parse_prefs_json(&json), p);
    }

    #[test]
    fn apply_lid_action_then_current_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::PowerLidAction,
                &SettingValue::from_serde(&"hibernate".to_string()).unwrap(),
            )
            .unwrap();
            let v: String = current(SettingKey::PowerLidAction)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(v, "hibernate");
        });
    }

    #[test]
    fn apply_idle_timeouts_round_trip_and_dont_clobber_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::PowerSuspendIdleBatteryS,
                &SettingValue::from_serde(&300_u64).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::PowerSuspendIdleAcS,
                &SettingValue::from_serde(&1800_u64).unwrap(),
            )
            .unwrap();
            let bat: u64 = current(SettingKey::PowerSuspendIdleBatteryS)
                .unwrap()
                .to_serde()
                .unwrap();
            let ac: u64 = current(SettingKey::PowerSuspendIdleAcS)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(bat, 300);
            assert_eq!(ac, 1800);
        });
    }

    #[test]
    fn apply_caffeine_on_then_off_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::PowerPresentationMode,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            let on: bool = current(SettingKey::PowerPresentationMode)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(on);
            apply(
                SettingKey::PowerPresentationMode,
                &SettingValue::from_serde(&false).unwrap(),
            )
            .unwrap();
            let on: bool = current(SettingKey::PowerPresentationMode)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!(!on);
        });
    }

    #[test]
    fn apply_rejects_non_power_key() {
        let v = SettingValue::from_serde(&"x".to_string()).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }

    #[test]
    fn current_returns_defaults_when_sidecar_missing() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            let lid: String = current(SettingKey::PowerLidAction)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(lid, "");
            let bat: u64 = current(SettingKey::PowerSuspendIdleBatteryS)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(bat, 0);
        });
    }
}
