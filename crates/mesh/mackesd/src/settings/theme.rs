//! Theme applier — v2.0.0 Phase C.1.
//!
//! Writes the matching GSettings keys via the `gsettings` CLI so
//! every libadwaita / GTK app on the box picks up theme + icon-theme
//! + accent changes immediately. The cosmic-config + cosmic-theme
//! pipeline lands alongside the Phase E.3 panel rewrite — until
//! then, GSettings is the single point of truth the GTK Workbench
//! panels read from.

use super::{SettingKey, SettingValue};

const SCHEMA: &str = "org.gnome.desktop.interface";

/// Per-key GSettings key name on the `SCHEMA` schema.
fn gsettings_key(key: SettingKey) -> Option<&'static str> {
    match key {
        SettingKey::ThemeName => Some("gtk-theme"),
        SettingKey::ThemeIconSet => Some("icon-theme"),
        SettingKey::ThemeAccent => Some("accent-color"),
        SettingKey::ThemeMode => Some("color-scheme"),
        _ => None,
    }
}

/// Map the locked `ThemeMode` values (`light` / `dark` / `auto`)
/// onto the GSettings color-scheme string. Returns the input
/// unchanged for unknown values so a typo surfaces as a GSettings
/// error rather than a silent no-op.
fn mode_to_color_scheme(s: &str) -> String {
    match s {
        "dark" => "prefer-dark".to_owned(),
        "light" => "prefer-light".to_owned(),
        "auto" => "default".to_owned(),
        other => other.to_owned(),
    }
}

/// Same map in reverse.
fn color_scheme_to_mode(s: &str) -> String {
    match s {
        "prefer-dark" => "dark".to_owned(),
        "prefer-light" => "light".to_owned(),
        "default" => "auto".to_owned(),
        other => other.to_owned(),
    }
}

/// Apply a `theme.*` setting via `gsettings set <SCHEMA> <key> <value>`.
///
/// # Errors
///
/// Returns an error when:
///   * the key isn't a theme key
///   * the value isn't a string
///   * `gsettings` isn't installed or returns non-zero
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    let Some(gskey) = gsettings_key(key) else {
        anyhow::bail!("theme: {key} is not a theme key");
    };
    let s: String = value.to_serde()?;
    let final_value = if matches!(key, SettingKey::ThemeMode) {
        mode_to_color_scheme(&s)
    } else {
        s
    };
    let out = std::process::Command::new("gsettings")
        .args(["set", SCHEMA, gskey, &final_value])
        .output()
        .map_err(|e| anyhow::anyhow!("theme: gsettings spawn failed: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "theme: gsettings set {gskey} '{final_value}' failed ({}): {}",
            out.status.code().map_or("?".to_string(), |c| c.to_string()),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Read the live value via `gsettings get <SCHEMA> <key>`.
///
/// # Errors
///
/// Returns an error on key-not-theme, spawn-failure, or non-zero exit.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let Some(gskey) = gsettings_key(key) else {
        anyhow::bail!("theme: {key} is not a theme key");
    };
    let out = std::process::Command::new("gsettings")
        .args(["get", SCHEMA, gskey])
        .output()
        .map_err(|e| anyhow::anyhow!("theme: gsettings spawn failed: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "theme: gsettings get {gskey} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    // gsettings wraps string output in single quotes; strip them.
    let unquoted = raw.trim_matches('\'').to_owned();
    let final_value = if matches!(key, SettingKey::ThemeMode) {
        color_scheme_to_mode(&unquoted)
    } else {
        unquoted
    };
    SettingValue::from_serde(&final_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsettings_key_covers_every_theme_variant() {
        assert_eq!(gsettings_key(SettingKey::ThemeName), Some("gtk-theme"));
        assert_eq!(gsettings_key(SettingKey::ThemeIconSet), Some("icon-theme"));
        assert_eq!(gsettings_key(SettingKey::ThemeAccent), Some("accent-color"));
        assert_eq!(gsettings_key(SettingKey::ThemeMode), Some("color-scheme"));
    }

    #[test]
    fn gsettings_key_returns_none_for_non_theme_key() {
        assert_eq!(gsettings_key(SettingKey::FontName), None);
        assert_eq!(gsettings_key(SettingKey::WallpaperPath), None);
    }

    #[test]
    fn mode_to_color_scheme_translates_known_values() {
        assert_eq!(mode_to_color_scheme("dark"), "prefer-dark");
        assert_eq!(mode_to_color_scheme("light"), "prefer-light");
        assert_eq!(mode_to_color_scheme("auto"), "default");
    }

    #[test]
    fn mode_to_color_scheme_passes_unknown_through() {
        assert_eq!(mode_to_color_scheme("typo"), "typo");
        assert_eq!(mode_to_color_scheme(""), "");
    }

    #[test]
    fn color_scheme_to_mode_round_trips_through_mode_to_color_scheme() {
        for m in ["dark", "light", "auto"] {
            let cs = mode_to_color_scheme(m);
            let back = color_scheme_to_mode(&cs);
            assert_eq!(back, m);
        }
    }
}
