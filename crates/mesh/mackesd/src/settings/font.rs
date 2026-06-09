//! Font applier — v2.0.0 Phase C.2.
//!
//! Writes font-name + monospace + hinting + antialias settings via
//! `gsettings`. The fontconfig path (writing
//! `~/.config/fontconfig/fonts.conf` + `fc-cache -r`) lands when
//! Phase C.2's full sweep across non-libadwaita apps ships; for
//! now GSettings + libadwaita coverage is the load-bearing path.

use super::{SettingKey, SettingValue};

const SCHEMA: &str = "org.gnome.desktop.interface";

fn gsettings_key(key: SettingKey) -> Option<&'static str> {
    match key {
        SettingKey::FontName => Some("font-name"),
        SettingKey::FontMonospace => Some("monospace-font-name"),
        SettingKey::FontHinting => Some("font-hinting"),
        SettingKey::FontAntialias => Some("font-antialiasing"),
        _ => None,
    }
}

/// Apply a `font.*` setting via `gsettings set`.
///
/// # Errors
///
/// Returns an error when the key isn't a font key, the value isn't
/// a string, or `gsettings` returns non-zero.
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    let Some(gskey) = gsettings_key(key) else {
        anyhow::bail!("font: {key} is not a font key");
    };
    let s: String = value.to_serde()?;
    let out = std::process::Command::new("gsettings")
        .args(["set", SCHEMA, gskey, &s])
        .output()
        .map_err(|e| anyhow::anyhow!("font: gsettings spawn failed: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "font: gsettings set {gskey} '{s}' failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Read the live value via `gsettings get`.
///
/// # Errors
///
/// Returns an error on key-not-font, spawn-failure, or non-zero exit.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let Some(gskey) = gsettings_key(key) else {
        anyhow::bail!("font: {key} is not a font key");
    };
    let out = std::process::Command::new("gsettings")
        .args(["get", SCHEMA, gskey])
        .output()
        .map_err(|e| anyhow::anyhow!("font: gsettings spawn failed: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "font: gsettings get {gskey} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let unquoted = raw.trim_matches('\'').to_owned();
    SettingValue::from_serde(&unquoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsettings_key_covers_every_font_variant() {
        assert_eq!(gsettings_key(SettingKey::FontName), Some("font-name"));
        assert_eq!(
            gsettings_key(SettingKey::FontMonospace),
            Some("monospace-font-name")
        );
        assert_eq!(gsettings_key(SettingKey::FontHinting), Some("font-hinting"));
        assert_eq!(
            gsettings_key(SettingKey::FontAntialias),
            Some("font-antialiasing")
        );
    }

    #[test]
    fn gsettings_key_returns_none_for_non_font_key() {
        assert_eq!(gsettings_key(SettingKey::ThemeName), None);
    }
}
