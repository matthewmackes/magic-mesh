//! v2.0.0 Phase A.1 (locked 2026-05-19) — settings surface that
//! replaces the xfconf/xfsettingsd stack.
//!
//! This module exposes the typed key/value vocabulary the rest of the
//! daemon uses to read and write every "settings knob" a user (or a
//! fleet revision) can twist: theme, fonts, displays, power, etc.
//!
//! This module exposes the `SettingKey` enum, the `SettingValue` payload
//! wrapper, the `Setting` row struct, and the `apply()` / `current()`
//! dispatcher. Each per-concern applier (`theme::apply`, `font::apply`, …)
//! is fully implemented (Phase A surface + Phase C backends both landed).
//!
//! The sync store API (rusqlite via `crate::store::with_transaction`)
//! is the durable side. The async DBus surface in `crate::ipc::settings`
//! reads/writes this module's API behind `org.mackes.Settings`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod automount;
pub mod autostart;
pub mod display;
pub mod font;
pub mod input;
pub mod keybinds;
pub mod notification;
pub mod power;
pub mod theme;
pub mod wallpaper;

/// Typed identifier for every settings knob mackesd can apply.
///
/// Each variant maps to a stable dot-notated string (e.g.
/// `theme.name`, `display.brightness`) for DBus serialization and
/// SQLite storage in the `settings` table. Adding a variant requires
/// matching arms in [`SettingKey::as_str`], [`FromStr::from_str`],
/// and the [`apply`] dispatcher below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    // --- theme ---
    /// GTK / libcosmic theme name (e.g. "Mackes-Carbon").
    ThemeName,
    /// Accent color, hex `#RRGGBB`.
    ThemeAccent,
    /// `dark` / `light` / `auto`.
    ThemeMode,
    /// Icon theme name (e.g. "Papirus-Dark").
    ThemeIconSet,

    // --- font ---
    /// Primary UI font name (e.g. "SF Pro Display 11").
    FontName,
    /// Monospace font name (e.g. "JetBrainsMono Nerd Font 10").
    FontMonospace,
    /// `none` / `slight` / `medium` / `full`.
    FontHinting,
    /// `none` / `grayscale` / `rgba`.
    FontAntialias,

    // --- display ---
    /// Comma-separated list of primary output names in priority order.
    DisplayPrimary,
    /// Brightness percentage 0..=100 of the focused output.
    DisplayBrightness,
    /// Fractional scale factor (0.5..=3.0).
    DisplayScale,
    /// Night-light enabled (`true` / `false`).
    DisplayNightLight,
    /// Night-light color-temperature in Kelvin.
    DisplayNightLightTemp,

    // --- power ---
    /// `nothing` / `suspend` / `hibernate` / `poweroff`.
    PowerLidAction,
    /// Idle timeout in seconds before suspending while on battery.
    PowerSuspendIdleBatteryS,
    /// Idle timeout in seconds before suspending while on AC.
    PowerSuspendIdleAcS,
    /// `power-saver` / `balanced` / `performance`.
    PowerProfile,
    /// "Caffeine" — block idle dim/lock (`true` / `false`).
    PowerPresentationMode,

    // --- notification ---
    /// Do-not-disturb (`true` / `false`).
    NotificationDoNotDisturb,
    /// `top-left` / `top-right` / `bottom-left` / `bottom-right` / `center`.
    NotificationLocation,
    /// Default expire-after milliseconds for transient notifications.
    NotificationDefaultExpireMs,

    // --- automount ---
    /// Auto-mount removable drives on insert.
    AutomountOnInsert,
    /// Auto-open file manager on mount.
    AutomountOpenOnMount,
    /// Auto-run autorun.sh / autorun.inf (default `false` for safety).
    AutomountAutorun,

    // --- wallpaper ---
    /// Wallpaper image path for the primary output.
    WallpaperPath,
    /// `stretch` / `fit` / `fill` / `center` / `tile`.
    WallpaperMode,

    // --- input (keyboard) ---
    /// Key-repeat delay in milliseconds before the first repeat
    /// (100..=2000). Applied via `swaymsg input type:keyboard
    /// repeat_delay <ms>`.
    KeyboardRepeatDelay,
    /// Key-repeat rate in characters per second once the delay
    /// elapses (1..=100). Applied via `swaymsg input type:keyboard
    /// repeat_rate <cps>`.
    KeyboardRepeatRate,
    /// XKB layout code(s), e.g. `us` or `us,de` (comma-separated).
    /// Applied via `swaymsg input type:keyboard xkb_layout <codes>`.
    KeyboardXkbLayout,

    // --- input (mouse / touchpad) ---
    /// libinput pointer acceleration, -1.0..=1.0 (0.0 = default).
    /// Applied via `swaymsg input type:pointer pointer_accel <v>`.
    MousePointerAccel,
    /// Reverse (natural) scrolling on touchpads (`true`/`false`).
    /// Applied via `swaymsg input type:touchpad natural_scroll <on>`.
    MouseNaturalScroll,
    /// Tap-to-click on touchpads (`true`/`false`).
    /// Applied via `swaymsg input type:touchpad tap <on>`.
    MouseTapToClick,
    /// Left-handed button mapping on pointers (`true`/`false`).
    /// Applied via `swaymsg input type:pointer left_handed <on>`.
    MouseLeftHanded,

    // --- keybinds ---
    /// JSON dict of binding -> command (rendered into
    /// `~/.config/sway/config.d/mackes-bindings.conf`).
    KeybindsMap,

    // --- autostart ---
    /// JSON list of `.desktop` IDs the user has hidden.
    AutostartHidden,
    /// JSON list of explicit auto-launch additions.
    AutostartExtra,
}

impl SettingKey {
    /// Dot-notated string used on DBus and in the SQLite `settings`
    /// table.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThemeName => "theme.name",
            Self::ThemeAccent => "theme.accent",
            Self::ThemeMode => "theme.mode",
            Self::ThemeIconSet => "theme.icon_set",
            Self::FontName => "font.name",
            Self::FontMonospace => "font.monospace",
            Self::FontHinting => "font.hinting",
            Self::FontAntialias => "font.antialias",
            Self::DisplayPrimary => "display.primary",
            Self::DisplayBrightness => "display.brightness",
            Self::DisplayScale => "display.scale",
            Self::DisplayNightLight => "display.night_light",
            Self::DisplayNightLightTemp => "display.night_light_temp",
            Self::PowerLidAction => "power.lid_action",
            Self::PowerSuspendIdleBatteryS => "power.suspend_idle_battery_s",
            Self::PowerSuspendIdleAcS => "power.suspend_idle_ac_s",
            Self::PowerProfile => "power.profile",
            Self::PowerPresentationMode => "power.presentation_mode",
            Self::NotificationDoNotDisturb => "notification.do_not_disturb",
            Self::NotificationLocation => "notification.location",
            Self::NotificationDefaultExpireMs => "notification.default_expire_ms",
            Self::AutomountOnInsert => "automount.on_insert",
            Self::AutomountOpenOnMount => "automount.open_on_mount",
            Self::AutomountAutorun => "automount.autorun",
            Self::WallpaperPath => "wallpaper.path",
            Self::WallpaperMode => "wallpaper.mode",
            Self::KeyboardRepeatDelay => "keyboard.repeat_delay",
            Self::KeyboardRepeatRate => "keyboard.repeat_rate",
            Self::KeyboardXkbLayout => "keyboard.xkb_layout",
            Self::MousePointerAccel => "mouse.pointer_accel",
            Self::MouseNaturalScroll => "mouse.natural_scroll",
            Self::MouseTapToClick => "mouse.tap_to_click",
            Self::MouseLeftHanded => "mouse.left_handed",
            Self::KeybindsMap => "keybinds.map",
            Self::AutostartHidden => "autostart.hidden",
            Self::AutostartExtra => "autostart.extra",
        }
    }

    /// Every variant in declaration order. Useful for `Snapshot`,
    /// schema validation, and the "reset to defaults" flow.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::ThemeName,
            Self::ThemeAccent,
            Self::ThemeMode,
            Self::ThemeIconSet,
            Self::FontName,
            Self::FontMonospace,
            Self::FontHinting,
            Self::FontAntialias,
            Self::DisplayPrimary,
            Self::DisplayBrightness,
            Self::DisplayScale,
            Self::DisplayNightLight,
            Self::DisplayNightLightTemp,
            Self::PowerLidAction,
            Self::PowerSuspendIdleBatteryS,
            Self::PowerSuspendIdleAcS,
            Self::PowerProfile,
            Self::PowerPresentationMode,
            Self::NotificationDoNotDisturb,
            Self::NotificationLocation,
            Self::NotificationDefaultExpireMs,
            Self::AutomountOnInsert,
            Self::AutomountOpenOnMount,
            Self::AutomountAutorun,
            Self::WallpaperPath,
            Self::WallpaperMode,
            Self::KeyboardRepeatDelay,
            Self::KeyboardRepeatRate,
            Self::KeyboardXkbLayout,
            Self::MousePointerAccel,
            Self::MouseNaturalScroll,
            Self::MouseTapToClick,
            Self::MouseLeftHanded,
            Self::KeybindsMap,
            Self::AutostartHidden,
            Self::AutostartExtra,
        ]
    }
}

impl fmt::Display for SettingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SettingKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        for &key in Self::all() {
            if key.as_str() == s {
                return Ok(key);
            }
        }
        Err(anyhow!("unknown setting key: {s}"))
    }
}

/// Wrapper around a serde_json `Value`. The applier modules narrow
/// to their expected concrete type at apply-time and surface a clear
/// error if the JSON doesn't fit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingValue(pub Value);

impl SettingValue {
    /// Construct from any serializable Rust value.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization to JSON fails. In practice
    /// this never happens for `Serialize` impls in this crate.
    pub fn from_serde<T: Serialize>(v: &T) -> anyhow::Result<Self> {
        Ok(Self(
            serde_json::to_value(v).context("serializing SettingValue")?,
        ))
    }

    /// Narrow to a concrete Rust type.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying JSON doesn't deserialize
    /// into `T`. Used by every applier to assert "this key expects
    /// a string / integer / struct".
    pub fn to_serde<T: for<'de> Deserialize<'de>>(&self) -> anyhow::Result<T> {
        serde_json::from_value(self.0.clone()).context("deserializing SettingValue")
    }
}

/// A single materialized row in the `settings` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    /// Typed key.
    pub key: SettingKey,
    /// Value payload.
    pub value: SettingValue,
    /// UTC instant the applier last successfully wrote this value.
    pub last_applied_at: DateTime<Utc>,
    /// Originating fleet revision, or `None` for locally-set values.
    pub source_revision_id: Option<String>,
}

/// A complete snapshot of every setting, used by
/// `org.mackes.Settings.Snapshot` / `.Restore`. `BTreeMap` so the
/// JSON output is deterministic (matters for diffs + golden tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Snapshot {
    /// All values, keyed by [`SettingKey::as_str`].
    pub values: BTreeMap<String, SettingValue>,
    /// UTC instant the snapshot was captured.
    pub captured_at: Option<DateTime<Utc>>,
}

/// Outcome of a single `apply` call. Per-key so a multi-key fleet
/// revision can partially succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    /// Which key the outcome refers to.
    pub key: SettingKey,
    /// `true` if the applier reported success.
    pub ok: bool,
    /// Free-form error message; `None` when `ok == true`.
    pub error: Option<String>,
}

/// Dispatcher: route a (key, value) pair to the applier owning it.
///
/// # Errors
///
/// Returns an error if the applier rejects the value (wrong shape,
/// out-of-range, or missing backend).
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    match key {
        SettingKey::ThemeName
        | SettingKey::ThemeAccent
        | SettingKey::ThemeMode
        | SettingKey::ThemeIconSet => theme::apply(key, value),
        SettingKey::FontName
        | SettingKey::FontMonospace
        | SettingKey::FontHinting
        | SettingKey::FontAntialias => font::apply(key, value),
        SettingKey::DisplayPrimary
        | SettingKey::DisplayBrightness
        | SettingKey::DisplayScale
        | SettingKey::DisplayNightLight
        | SettingKey::DisplayNightLightTemp => display::apply(key, value),
        SettingKey::PowerLidAction
        | SettingKey::PowerSuspendIdleBatteryS
        | SettingKey::PowerSuspendIdleAcS
        | SettingKey::PowerProfile
        | SettingKey::PowerPresentationMode => power::apply(key, value),
        SettingKey::NotificationDoNotDisturb
        | SettingKey::NotificationLocation
        | SettingKey::NotificationDefaultExpireMs => notification::apply(key, value),
        SettingKey::AutomountOnInsert
        | SettingKey::AutomountOpenOnMount
        | SettingKey::AutomountAutorun => automount::apply(key, value),
        SettingKey::WallpaperPath | SettingKey::WallpaperMode => wallpaper::apply(key, value),
        SettingKey::KeyboardRepeatDelay
        | SettingKey::KeyboardRepeatRate
        | SettingKey::KeyboardXkbLayout
        | SettingKey::MousePointerAccel
        | SettingKey::MouseNaturalScroll
        | SettingKey::MouseTapToClick
        | SettingKey::MouseLeftHanded => input::apply(key, value),
        SettingKey::KeybindsMap => keybinds::apply(key, value),
        SettingKey::AutostartHidden | SettingKey::AutostartExtra => autostart::apply(key, value),
    }
}

/// Dispatcher: read the current value an applier sees in the live
/// system (so the GUI can show what's actually applied, not just
/// what's in the database).
///
/// # Errors
///
/// Returns an error if the applier hasn't shipped its real
/// implementation, or if the backend it queries (GSettings, sway IPC,
/// login1, ...) fails.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    match key {
        SettingKey::ThemeName
        | SettingKey::ThemeAccent
        | SettingKey::ThemeMode
        | SettingKey::ThemeIconSet => theme::current(key),
        SettingKey::FontName
        | SettingKey::FontMonospace
        | SettingKey::FontHinting
        | SettingKey::FontAntialias => font::current(key),
        SettingKey::DisplayPrimary
        | SettingKey::DisplayBrightness
        | SettingKey::DisplayScale
        | SettingKey::DisplayNightLight
        | SettingKey::DisplayNightLightTemp => display::current(key),
        SettingKey::PowerLidAction
        | SettingKey::PowerSuspendIdleBatteryS
        | SettingKey::PowerSuspendIdleAcS
        | SettingKey::PowerProfile
        | SettingKey::PowerPresentationMode => power::current(key),
        SettingKey::NotificationDoNotDisturb
        | SettingKey::NotificationLocation
        | SettingKey::NotificationDefaultExpireMs => notification::current(key),
        SettingKey::AutomountOnInsert
        | SettingKey::AutomountOpenOnMount
        | SettingKey::AutomountAutorun => automount::current(key),
        SettingKey::WallpaperPath | SettingKey::WallpaperMode => wallpaper::current(key),
        SettingKey::KeyboardRepeatDelay
        | SettingKey::KeyboardRepeatRate
        | SettingKey::KeyboardXkbLayout
        | SettingKey::MousePointerAccel
        | SettingKey::MouseNaturalScroll
        | SettingKey::MouseTapToClick
        | SettingKey::MouseLeftHanded => input::current(key),
        SettingKey::KeybindsMap => keybinds::current(key),
        SettingKey::AutostartHidden | SettingKey::AutostartExtra => autostart::current(key),
    }
}

/// v2.0.0 Phase G.2 — apply every (key, value_json) pair from a
/// fleet revision. Returns one `ApplyOutcome` per pair so callers
/// (the reconcile loop, the Workbench Fleet panel) can render
/// per-key results.
///
/// Errors do NOT short-circuit — every pair is attempted so the
/// operator sees the full failure picture in one tick.
#[must_use]
pub fn apply_all(pairs: &[(String, String)]) -> Vec<ApplyOutcome> {
    let mut outcomes = Vec::with_capacity(pairs.len());
    for (key_str, value_json) in pairs {
        let Ok(key): Result<SettingKey, _> = key_str.parse() else {
            outcomes.push(ApplyOutcome {
                key: SettingKey::ThemeName, // placeholder — error overrides
                ok: false,
                error: Some(format!("unknown setting key: {key_str}")),
            });
            continue;
        };
        let value: SettingValue = match serde_json::from_str(value_json) {
            Ok(v) => v,
            Err(e) => {
                outcomes.push(ApplyOutcome {
                    key,
                    ok: false,
                    error: Some(format!("value_json: {e}")),
                });
                continue;
            }
        };
        match apply(key, &value) {
            Ok(()) => outcomes.push(ApplyOutcome {
                key,
                ok: true,
                error: None,
            }),
            Err(e) => outcomes.push(ApplyOutcome {
                key,
                ok: false,
                error: Some(format!("{e:#}")),
            }),
        }
    }
    outcomes
}

#[cfg(test)]
mod g2_tests {
    use super::*;

    #[test]
    fn apply_all_handles_empty_input() {
        assert!(apply_all(&[]).is_empty());
    }

    #[test]
    fn apply_all_reports_unknown_key_per_entry() {
        let outcomes = apply_all(&[("not.a.real.key".into(), r#""x""#.into())]);
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].ok);
        assert!(outcomes[0]
            .error
            .as_ref()
            .unwrap()
            .contains("unknown setting key"));
    }

    #[test]
    fn apply_all_reports_malformed_json_per_entry() {
        let outcomes = apply_all(&[("theme.name".into(), "{not json".into())]);
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].ok);
        assert!(outcomes[0].error.as_ref().unwrap().contains("value_json"));
    }

    #[test]
    fn apply_all_does_not_short_circuit_on_first_error() {
        let outcomes = apply_all(&[
            ("not.a.key".into(), r#""x""#.into()),
            ("also.bogus".into(), r#""y""#.into()),
        ]);
        assert_eq!(outcomes.len(), 2, "every pair attempted");
        assert!(outcomes.iter().all(|o| !o.ok));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_round_trips_through_string() {
        for &key in SettingKey::all() {
            let s = key.as_str();
            let parsed: SettingKey = s.parse().unwrap();
            assert_eq!(parsed, key, "round-trip failed for {s}");
        }
    }

    #[test]
    fn from_str_rejects_unknown_keys() {
        let r: anyhow::Result<SettingKey> = "not.a.real.key".parse();
        assert!(r.is_err());
    }

    #[test]
    fn keys_are_dot_notated_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for &key in SettingKey::all() {
            let s = key.as_str();
            assert!(s.contains('.'), "key {s} is not dot-notated");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "key {s} has invalid chars"
            );
            assert!(seen.insert(s), "duplicate key {s}");
        }
    }

    #[test]
    fn setting_value_round_trips_through_serde() {
        let v = SettingValue::from_serde(&"hello").unwrap();
        let back: String = v.to_serde().unwrap();
        assert_eq!(back, "hello");
    }

    #[test]
    fn setting_value_rejects_wrong_type() {
        let v = SettingValue::from_serde(&42_u32).unwrap();
        let r: anyhow::Result<String> = v.to_serde();
        assert!(r.is_err());
    }

    #[test]
    fn apply_dispatcher_covers_every_key() {
        // Smoke: every SettingKey has a handler in the dispatcher
        // (none panics, none routes to nothing).
        for &key in SettingKey::all() {
            // Use a type-mismatching value so apply() returns a
            // shape-validation error rather than actually mutating
            // state. We only care that the dispatcher finds a
            // handler.
            let value = SettingValue::from_serde(&serde_json::Value::Null).unwrap();
            let _ = apply(key, &value);
        }
    }

    #[test]
    fn snapshot_is_deterministic() {
        let mut a = Snapshot::default();
        let mut b = Snapshot::default();
        for &key in &[
            SettingKey::ThemeAccent,
            SettingKey::ThemeName,
            SettingKey::FontName,
        ] {
            a.values.insert(
                key.as_str().to_string(),
                SettingValue::from_serde(&"x").unwrap(),
            );
            b.values.insert(
                key.as_str().to_string(),
                SettingValue::from_serde(&"x").unwrap(),
            );
        }
        let a_json = serde_json::to_string(&a).unwrap();
        let b_json = serde_json::to_string(&b).unwrap();
        assert_eq!(
            a_json, b_json,
            "BTreeMap serialization must be order-stable"
        );
    }
}
