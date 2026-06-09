//! Input applier — keyboard (+ later pointer/touchpad) settings.
//!
//! Replaces the v1.x `mackes/workbench/devices/keyboard.py` (and
//! `mouse.py`) xfconf surface. Three keyboard keys today —
//! `keyboard.repeat_delay`, `keyboard.repeat_rate`,
//! `keyboard.xkb_layout` — each:
//!
//!   1. persists to a JSON sidecar at `$XDG_CACHE_HOME/mde/input.json`
//!      (mirrors [`super::display`]'s `DisplayPrefs` pattern), so the
//!      value survives and a future mde-session login hook can
//!      re-apply it; and
//!   2. best-effort live-applies via `swaymsg input type:keyboard
//!      <subcmd> <value>` so the change takes effect immediately on a
//!      running sway session. A missing `swaymsg` / no live session
//!      degrades gracefully — the sidecar write is the source of truth
//!      and the runtime apply is logged-but-not-fatal (a headless box
//!      or TTY must still be able to set the value).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SettingKey, SettingValue};

/// JSON sidecar shape for the persisted input keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputPrefs {
    /// Key-repeat delay in milliseconds (100..=2000).
    #[serde(default = "default_repeat_delay")]
    pub repeat_delay_ms: u32,
    /// Key-repeat rate in characters per second (1..=100).
    #[serde(default = "default_repeat_rate")]
    pub repeat_rate_cps: u32,
    /// XKB layout code(s), e.g. `us` or `us,de`.
    #[serde(default = "default_xkb_layout")]
    pub xkb_layout: String,
    /// libinput pointer acceleration, -1.0..=1.0 (0.0 = system default).
    #[serde(default)]
    pub pointer_accel: f64,
    /// Reverse (natural) scrolling on touchpads.
    #[serde(default)]
    pub natural_scroll: bool,
    /// Tap-to-click on touchpads.
    #[serde(default)]
    pub tap_to_click: bool,
    /// Left-handed button mapping on pointers.
    #[serde(default)]
    pub left_handed: bool,
}

impl Default for InputPrefs {
    fn default() -> Self {
        Self {
            repeat_delay_ms: default_repeat_delay(),
            repeat_rate_cps: default_repeat_rate(),
            xkb_layout: default_xkb_layout(),
            pointer_accel: 0.0,
            natural_scroll: false,
            tap_to_click: false,
            left_handed: false,
        }
    }
}

const fn default_repeat_delay() -> u32 {
    600
}
const fn default_repeat_rate() -> u32 {
    25
}
fn default_xkb_layout() -> String {
    "us".to_owned()
}

const REPEAT_DELAY_RANGE: std::ops::RangeInclusive<u32> = 100..=2000;
const REPEAT_RATE_RANGE: std::ops::RangeInclusive<u32> = 1..=100;
const POINTER_ACCEL_RANGE: std::ops::RangeInclusive<f64> = -1.0..=1.0;

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
    cache_root().join("mde").join("input.json")
}

/// Pure helper: parse the sidecar; default on malformed.
#[must_use]
pub fn parse_prefs_json(text: &str) -> InputPrefs {
    serde_json::from_str(text).unwrap_or_default()
}

/// Pure helper: the `swaymsg` `(input-type, subcommand)` pair for a
/// key. `None` for keys this module doesn't own.
#[must_use]
pub fn sway_input_target(key: SettingKey) -> Option<(&'static str, &'static str)> {
    match key {
        SettingKey::KeyboardRepeatDelay => Some(("type:keyboard", "repeat_delay")),
        SettingKey::KeyboardRepeatRate => Some(("type:keyboard", "repeat_rate")),
        SettingKey::KeyboardXkbLayout => Some(("type:keyboard", "xkb_layout")),
        SettingKey::MousePointerAccel => Some(("type:pointer", "pointer_accel")),
        SettingKey::MouseLeftHanded => Some(("type:pointer", "left_handed")),
        SettingKey::MouseNaturalScroll => Some(("type:touchpad", "natural_scroll")),
        SettingKey::MouseTapToClick => Some(("type:touchpad", "tap")),
        _ => None,
    }
}

/// Pure helper: the full `swaymsg` argv that applies `key`'s `value`
/// to a live session. `None` for keys this module doesn't own.
#[must_use]
pub fn sway_input_args(key: SettingKey, value: &str) -> Option<Vec<String>> {
    let (input_type, subcmd) = sway_input_target(key)?;
    Some(vec![
        "input".to_owned(),
        input_type.to_owned(),
        subcmd.to_owned(),
        value.to_owned(),
    ])
}

/// libinput boolean knobs take `enabled` / `disabled`, not `true` /
/// `false`.
#[must_use]
fn bool_to_sway(b: bool) -> &'static str {
    if b {
        "enabled"
    } else {
        "disabled"
    }
}

/// Apply a `keyboard.*` setting: validate, persist to the sidecar,
/// then best-effort live-apply via `swaymsg`.
///
/// # Errors
/// Returns an error when the key isn't an input key, the value fails
/// to deserialize, the value is out of range, or the sidecar write
/// fails. A failed live `swaymsg` apply is logged, not returned — the
/// persisted value is authoritative.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    let applied_value: String = match key {
        SettingKey::KeyboardRepeatDelay => {
            let ms: u32 = value.to_serde()?;
            if !REPEAT_DELAY_RANGE.contains(&ms) {
                anyhow::bail!("input: keyboard repeat_delay must be 100..=2000 ms, got {ms}");
            }
            update_prefs(move |p| p.repeat_delay_ms = ms)?;
            ms.to_string()
        }
        SettingKey::KeyboardRepeatRate => {
            let cps: u32 = value.to_serde()?;
            if !REPEAT_RATE_RANGE.contains(&cps) {
                anyhow::bail!("input: keyboard repeat_rate must be 1..=100 cps, got {cps}");
            }
            update_prefs(move |p| p.repeat_rate_cps = cps)?;
            cps.to_string()
        }
        SettingKey::KeyboardXkbLayout => {
            let layout: String = value.to_serde()?;
            if layout.trim().is_empty() {
                anyhow::bail!("input: keyboard xkb_layout must be a non-empty code (e.g. us)");
            }
            let layout_clone = layout.clone();
            update_prefs(move |p| p.xkb_layout = layout_clone.clone())?;
            layout
        }
        SettingKey::MousePointerAccel => {
            let accel: f64 = value.to_serde()?;
            if !POINTER_ACCEL_RANGE.contains(&accel) {
                anyhow::bail!("input: mouse pointer_accel must be -1.0..=1.0, got {accel}");
            }
            update_prefs(move |p| p.pointer_accel = accel)?;
            // sway accepts e.g. `0.5`; format without trailing noise.
            format!("{accel}")
        }
        SettingKey::MouseNaturalScroll => {
            let on: bool = value.to_serde()?;
            update_prefs(move |p| p.natural_scroll = on)?;
            bool_to_sway(on).to_owned()
        }
        SettingKey::MouseTapToClick => {
            let on: bool = value.to_serde()?;
            update_prefs(move |p| p.tap_to_click = on)?;
            bool_to_sway(on).to_owned()
        }
        SettingKey::MouseLeftHanded => {
            let on: bool = value.to_serde()?;
            update_prefs(move |p| p.left_handed = on)?;
            bool_to_sway(on).to_owned()
        }
        _ => anyhow::bail!("input: {key} is not an input key"),
    };

    // Best-effort live apply — failure is non-fatal (headless / TTY /
    // no running sway session). The sidecar write above is the source
    // of truth.
    if let Some(args) = sway_input_args(key, &applied_value) {
        match std::process::Command::new("swaymsg").args(&args).output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => tracing::debug!(
                "input: swaymsg {args:?} returned non-zero: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => tracing::debug!("input: swaymsg spawn failed (no live session?): {e}"),
        }
    }
    Ok(())
}

fn update_prefs(mut mutator: impl FnMut(&mut InputPrefs)) -> anyhow::Result<()> {
    let path = prefs_path();
    let mut prefs = std::fs::read_to_string(&path)
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default();
    mutator(&mut prefs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("input: mkdir {} failed: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&prefs)
        .map_err(|e| anyhow::anyhow!("input: serialize: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| anyhow::anyhow!("input: write {} failed: {e}", path.display()))
}

fn read_prefs() -> InputPrefs {
    std::fs::read_to_string(prefs_path())
        .map(|s| parse_prefs_json(&s))
        .unwrap_or_default()
}

/// Read the current `keyboard.*` setting from the sidecar.
///
/// # Errors
/// Returns an error when the key isn't an input key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let prefs = read_prefs();
    match key {
        SettingKey::KeyboardRepeatDelay => SettingValue::from_serde(&prefs.repeat_delay_ms),
        SettingKey::KeyboardRepeatRate => SettingValue::from_serde(&prefs.repeat_rate_cps),
        SettingKey::KeyboardXkbLayout => SettingValue::from_serde(&prefs.xkb_layout),
        SettingKey::MousePointerAccel => SettingValue::from_serde(&prefs.pointer_accel),
        SettingKey::MouseNaturalScroll => SettingValue::from_serde(&prefs.natural_scroll),
        SettingKey::MouseTapToClick => SettingValue::from_serde(&prefs.tap_to_click),
        SettingKey::MouseLeftHanded => SettingValue::from_serde(&prefs.left_handed),
        _ => anyhow::bail!("input: {key} is not an input key"),
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
    fn parse_prefs_json_default_values() {
        let p = parse_prefs_json("");
        assert_eq!(p.repeat_delay_ms, 600);
        assert_eq!(p.repeat_rate_cps, 25);
        assert_eq!(p.xkb_layout, "us");
    }

    #[test]
    fn sway_input_args_maps_each_keyboard_key() {
        assert_eq!(
            sway_input_args(SettingKey::KeyboardRepeatDelay, "600"),
            Some(vec![
                "input".into(),
                "type:keyboard".into(),
                "repeat_delay".into(),
                "600".into()
            ])
        );
        assert_eq!(
            sway_input_args(SettingKey::KeyboardRepeatRate, "25").unwrap()[2],
            "repeat_rate"
        );
        assert_eq!(
            sway_input_args(SettingKey::KeyboardXkbLayout, "us,de").unwrap()[3],
            "us,de"
        );
    }

    #[test]
    fn sway_input_args_returns_none_for_non_input_key() {
        assert!(sway_input_args(SettingKey::ThemeName, "x").is_none());
    }

    #[test]
    fn apply_repeat_delay_rejects_out_of_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            assert!(apply(
                SettingKey::KeyboardRepeatDelay,
                &SettingValue::from_serde(&50_u32).unwrap()
            )
            .is_err());
            assert!(apply(
                SettingKey::KeyboardRepeatDelay,
                &SettingValue::from_serde(&5000_u32).unwrap()
            )
            .is_err());
        });
    }

    #[test]
    fn apply_repeat_rate_rejects_out_of_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            assert!(apply(
                SettingKey::KeyboardRepeatRate,
                &SettingValue::from_serde(&0_u32).unwrap()
            )
            .is_err());
            assert!(apply(
                SettingKey::KeyboardRepeatRate,
                &SettingValue::from_serde(&200_u32).unwrap()
            )
            .is_err());
        });
    }

    #[test]
    fn apply_xkb_layout_rejects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            assert!(apply(
                SettingKey::KeyboardXkbLayout,
                &SettingValue::from_serde(&"  ").unwrap()
            )
            .is_err());
        });
    }

    #[test]
    fn apply_round_trips_through_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::KeyboardRepeatDelay,
                &SettingValue::from_serde(&300_u32).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::KeyboardRepeatRate,
                &SettingValue::from_serde(&40_u32).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::KeyboardXkbLayout,
                &SettingValue::from_serde(&"gb").unwrap(),
            )
            .unwrap();

            let delay: u32 = current(SettingKey::KeyboardRepeatDelay)
                .unwrap()
                .to_serde()
                .unwrap();
            let rate: u32 = current(SettingKey::KeyboardRepeatRate)
                .unwrap()
                .to_serde()
                .unwrap();
            let layout: String = current(SettingKey::KeyboardXkbLayout)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(delay, 300);
            assert_eq!(rate, 40);
            assert_eq!(layout, "gb");
        });
    }

    #[test]
    fn apply_one_key_preserves_others() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::KeyboardRepeatRate,
                &SettingValue::from_serde(&50_u32).unwrap(),
            )
            .unwrap();
            // The other two keep their defaults.
            let delay: u32 = current(SettingKey::KeyboardRepeatDelay)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(delay, 600);
        });
    }

    #[test]
    fn apply_rejects_non_input_key() {
        let v = SettingValue::from_serde(&1_u32).unwrap();
        assert!(apply(SettingKey::ThemeName, &v).is_err());
    }

    #[test]
    fn current_rejects_non_input_key() {
        assert!(current(SettingKey::ThemeName).is_err());
    }

    #[test]
    fn sway_input_target_routes_mouse_keys_to_pointer_or_touchpad() {
        assert_eq!(
            sway_input_target(SettingKey::MousePointerAccel),
            Some(("type:pointer", "pointer_accel"))
        );
        assert_eq!(
            sway_input_target(SettingKey::MouseLeftHanded),
            Some(("type:pointer", "left_handed"))
        );
        assert_eq!(
            sway_input_target(SettingKey::MouseNaturalScroll),
            Some(("type:touchpad", "natural_scroll"))
        );
        assert_eq!(
            sway_input_target(SettingKey::MouseTapToClick),
            Some(("type:touchpad", "tap"))
        );
    }

    #[test]
    fn bool_to_sway_uses_enabled_disabled() {
        assert_eq!(bool_to_sway(true), "enabled");
        assert_eq!(bool_to_sway(false), "disabled");
    }

    #[test]
    fn apply_pointer_accel_rejects_out_of_range() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            assert!(apply(
                SettingKey::MousePointerAccel,
                &SettingValue::from_serde(&2.0_f64).unwrap()
            )
            .is_err());
            assert!(apply(
                SettingKey::MousePointerAccel,
                &SettingValue::from_serde(&-1.5_f64).unwrap()
            )
            .is_err());
        });
    }

    #[test]
    fn apply_mouse_keys_round_trip_through_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::MousePointerAccel,
                &SettingValue::from_serde(&0.5_f64).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::MouseNaturalScroll,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::MouseTapToClick,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::MouseLeftHanded,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();

            let accel: f64 = current(SettingKey::MousePointerAccel)
                .unwrap()
                .to_serde()
                .unwrap();
            let natural: bool = current(SettingKey::MouseNaturalScroll)
                .unwrap()
                .to_serde()
                .unwrap();
            let tap: bool = current(SettingKey::MouseTapToClick)
                .unwrap()
                .to_serde()
                .unwrap();
            let left: bool = current(SettingKey::MouseLeftHanded)
                .unwrap()
                .to_serde()
                .unwrap();
            assert!((accel - 0.5).abs() < f64::EPSILON);
            assert!(natural && tap && left);
        });
    }

    #[test]
    fn apply_mouse_key_preserves_keyboard_keys() {
        let tmp = tempfile::tempdir().unwrap();
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::KeyboardRepeatRate,
                &SettingValue::from_serde(&50_u32).unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::MouseLeftHanded,
                &SettingValue::from_serde(&true).unwrap(),
            )
            .unwrap();
            let rate: u32 = current(SettingKey::KeyboardRepeatRate)
                .unwrap()
                .to_serde()
                .unwrap();
            assert_eq!(rate, 50);
        });
    }
}
