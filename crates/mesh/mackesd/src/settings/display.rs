//! Display applier — v2.0.0 Phase C.3.
//!
//! Five keys span two control surfaces:
//!
//!   1. `DisplayBrightness` — shells out to `brightnessctl set N%`
//!      (kernel DRM API, works on X11 + Wayland). Read path:
//!      `brightnessctl get` / `brightnessctl max` (we compute the
//!      percentage).
//!   2. `DisplayPrimary` / `DisplayScale` / `DisplayNightLight` /
//!      `DisplayNightLightTemp` — persist to a JSON sidecar at
//!      `$XDG_CACHE_HOME/mde/display.json`. mde-session (Phase D)
//!      reads the sidecar and re-applies via swaymsg /
//!      wlr-output-management / gammastep on each login.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON sidecar shape for the keys persisted to disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayPrefs {
    /// Comma-separated list of primary output names in priority order.
    #[serde(default)]
    pub primary: String,
    /// Fractional scale factor (0.5..=3.0).
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Night-light enabled.
    #[serde(default)]
    pub night_light: bool,
    /// Night-light color-temperature in Kelvin.
    #[serde(default = "default_night_temp")]
    pub night_light_temp: u32,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        Self {
            primary: String::new(),
            scale: default_scale(),
            night_light: false,
            night_light_temp: default_night_temp(),
        }
    }
}

const fn default_scale() -> f64 {
    1.0
}
const fn default_night_temp() -> u32 {
    4500
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
    cache_root().join("mde").join("display.json")
}

/// Pure helper: parse the sidecar; default on malformed.
#[must_use]
pub fn parse_prefs_json(text: &str) -> DisplayPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Pure helper: compute brightness percent from current/max readings.
/// Returns `None` when `max == 0` (degenerate).
#[must_use]
pub fn brightness_percent(current: u64, max: u64) -> Option<u8> {
    if max == 0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let pct = ((current as f64 / max as f64) * 100.0).round();
    Some(pct.clamp(0.0, 100.0) as u8)
}

/// Apply a `display.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a display key, the value
/// fails to deserialize, or the subprocess / filesystem call fails.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    match key {
        SettingKey::DisplayBrightness => {
            let pct: u8 = value.to_serde()?;
            if pct > 100 {
                anyhow::bail!("display: brightness must be 0..=100, got {pct}");
            }
            let out = std::process::Command::new("brightnessctl")
                .args(["set", &format!("{pct}%")])
                .output()
                .map_err(|e| anyhow::anyhow!("display: brightnessctl spawn failed: {e}"))?;
            if !out.status.success() {
                anyhow::bail!(
                    "display: brightnessctl set {pct}% failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(())
        }
        SettingKey::DisplayPrimary => {
            let name: String = value.to_serde()?;
            update_prefs(move |p| p.primary = name.clone())
        }
        SettingKey::DisplayScale => {
            let scale: f64 = value.to_serde()?;
            if !(0.5..=3.0).contains(&scale) {
                anyhow::bail!("display: scale must be 0.5..=3.0, got {scale}");
            }
            update_prefs(move |p| p.scale = scale)
        }
        SettingKey::DisplayNightLight => {
            let on: bool = value.to_serde()?;
            update_prefs(move |p| p.night_light = on)
        }
        SettingKey::DisplayNightLightTemp => {
            let kelvin: u32 = value.to_serde()?;
            if !(1000..=10_000).contains(&kelvin) {
                anyhow::bail!("display: night-light temp must be 1000..=10000 K, got {kelvin}");
            }
            update_prefs(move |p| p.night_light_temp = kelvin)
        }
        _ => anyhow::bail!("display: {key} is not a display key"),
    }
}

fn update_prefs(mut mutator: impl FnMut(&mut DisplayPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = std::fs::read_to_string(&path)
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("display: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("display: serialize: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("display: write {} failed: {e}", path.display()))
}

/// Read the current `display.*` setting.
///
/// # Errors
/// Returns an error when the key isn't a display key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    match key {
        SettingKey::DisplayBrightness => {
            let get = std::process::Command::new("brightnessctl")
                .arg("get")
                .output()
                .map_err(|e| anyhow::anyhow!("display: brightnessctl get failed: {e}"))?;
            let max = std::process::Command::new("brightnessctl")
                .arg("max")
                .output()
                .map_err(|e| anyhow::anyhow!("display: brightnessctl max failed: {e}"))?;
            if !get.status.success() || !max.status.success() {
                anyhow::bail!("display: brightnessctl returned non-zero");
            }
            let cur: u64 = String::from_utf8_lossy(&get.stdout)
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("display: brightnessctl get parse: {e}"))?;
            let m: u64 = String::from_utf8_lossy(&max.stdout)
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("display: brightnessctl max parse: {e}"))?;
            let pct = brightness_percent(cur, m).unwrap_or(0);
            SettingValue::from_serde(&pct)
        }
        SettingKey::DisplayPrimary => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.primary)
        }
        SettingKey::DisplayScale => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.scale)
        }
        SettingKey::DisplayNightLight => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.night_light)
        }
        SettingKey::DisplayNightLightTemp => {
            let prefs = read_prefs();
            SettingValue::from_serde(&prefs.night_light_temp)
        }
        _ => anyhow::bail!("display: {key} is not a display key"),
    }
}

fn read_prefs() -> DisplayPrefs {
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
    fn brightness_percent_handles_typical_values() {
        assert_eq!(brightness_percent(0, 100), Some(0));
        assert_eq!(brightness_percent(50, 100), Some(50));
        assert_eq!(brightness_percent(100, 100), Some(100));
        assert_eq!(brightness_percent(150, 200), Some(75));
    }

    #[test]
    fn brightness_percent_returns_none_for_zero_max() {
        assert!(brightness_percent(0, 0).is_none());
    }

    #[test]
    fn brightness_percent_clamps_to_100() {
        assert_eq!(brightness_percent(200, 100), Some(100));
    }

    #[test]
    fn parse_prefs_json_default_values() {
        let p = parse_prefs_json("");
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.night_light_temp, 4500);
    }

    #[test]
    fn apply_scale_rejects_out_of_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            let r = apply(
                SettingKey::DisplayScale,
                &SettingValue::from_serde(&5.0_f64).unwrap(),
            );
            assert!(r.is_err());
            let r = apply(
                SettingKey::DisplayScale,
                &SettingValue::from_serde(&0.1_f64).unwrap(),
            );
            assert!(r.is_err());
        });
    }

    #[test]
    fn apply_scale_accepts_in_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::DisplayScale,
                &SettingValue::from_serde(&1.5_f64).unwrap(),
            )
            .unwrap();
            let scale: f64 = current(SettingKey::DisplayScale)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(scale, 1.5);
        });
    }

    #[test]
    fn apply_primary_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::DisplayPrimary,
                &SettingValue::from_serde(&"DP-1,eDP-1".to_string()).unwrap(),
            )
            .unwrap();
            let s: String = current(SettingKey::DisplayPrimary)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(s, "DP-1,eDP-1");
        });
    }

    #[test]
    fn apply_night_light_temp_rejects_out_of_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            let r = apply(
                SettingKey::DisplayNightLightTemp,
                &SettingValue::from_serde(&500_u32).unwrap(),
            );
            assert!(r.is_err());
            let r = apply(
                SettingKey::DisplayNightLightTemp,
                &SettingValue::from_serde(&15_000_u32).unwrap(),
            );
            assert!(r.is_err());
        });
    }

    #[test]
    fn apply_night_light_temp_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::DisplayNightLightTemp,
                &SettingValue::from_serde(&3500_u32).unwrap(),
            )
            .unwrap();
            let k: u32 = current(SettingKey::DisplayNightLightTemp)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(k, 3500);
        });
    }

    #[test]
    fn apply_brightness_rejects_above_100() {
        let r = apply(
            SettingKey::DisplayBrightness,
            &SettingValue::from_serde(&101_u8).unwrap(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn apply_rejects_non_display_key() {
        let v = SettingValue::from_serde(&1.0_f64).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }

    #[test]
    fn apply_one_display_key_preserves_others() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::DisplayScale,
                &SettingValue::from_serde(&2.0_f64).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::DisplayNightLight,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            let scale: f64 = current(SettingKey::DisplayScale)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(scale, 2.0);
        });
    }
}
