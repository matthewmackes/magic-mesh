//! Autostart applier — v2.0.0 Phase C.9.
//!
//! Toggles `Hidden=true` on `~/.config/autostart/<id>.desktop` to
//! suppress system-wide autostart entries on a per-user basis. The
//! `autostart.hidden` value carries a JSON list of `.desktop` IDs
//! the user wants hidden; the `autostart.extra` value carries a JSON
//! list of `.desktop` IDs the user wants explicitly enabled (we
//! write a one-line `.desktop` file pointing at the matching system
//! entry with `Hidden=false`).
//!
//! Pure-function helpers (`load_autostart_state`, `desktop_id_path`,
//! `hidden_overlay_text`) live alongside the impl so the file-format
//! contract is unit-testable without touching `~/.config`.

use std::path::PathBuf;

use super::{SettingKey, SettingValue};

/// `.desktop` ID list payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct AutostartList {
    /// Stable identifiers of `.desktop` entries (without `.desktop`
    /// extension).
    pub ids: Vec<String>,
}

/// Resolve `~/.config/autostart/`, honoring `$XDG_CONFIG_HOME`.
#[must_use]
pub fn autostart_dir() -> PathBuf {
    if let Ok(s) = std::env::var("XDG_CONFIG_HOME") {
        if !s.is_empty() {
            return PathBuf::from(s).join("autostart");
        }
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".config").join("autostart");
    }
    PathBuf::from(".")
}

/// Path of the per-user override `.desktop` for the given id.
#[must_use]
pub fn desktop_id_path(id: &str) -> PathBuf {
    autostart_dir().join(format!("{id}.desktop"))
}

/// Content of the minimal `Hidden=true` overlay `.desktop` we write
/// to suppress an autostart entry.
#[must_use]
pub fn hidden_overlay_text(id: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={id}\n\
         Hidden=true\n\
         X-MDE-Generated=true\n"
    )
}

/// Apply an `autostart.*` setting.
///
/// # Errors
///
/// Returns an error when:
///   * the key isn't an autostart key
///   * the value isn't an `AutostartList`
///   * filesystem writes fail
pub fn apply(key: SettingKey, value: &SettingValue) -> anyhow::Result<()> {
    let list: AutostartList = value.to_serde()?;
    match key {
        SettingKey::AutostartHidden => apply_hidden(&list),
        SettingKey::AutostartExtra => apply_extra(&list),
        _ => anyhow::bail!("autostart: {key} is not an autostart key"),
    }
}

fn apply_hidden(list: &AutostartList) -> anyhow::Result<()> {
    let dir = autostart_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("autostart: mkdir {} failed: {e}", dir.display()))?;
    for id in &list.ids {
        let path = desktop_id_path(id);
        let text = hidden_overlay_text(id);
        std::fs::write(&path, text)
            .map_err(|e| anyhow::anyhow!("autostart: write {} failed: {e}", path.display()))?;
    }
    Ok(())
}

fn apply_extra(list: &AutostartList) -> anyhow::Result<()> {
    let dir = autostart_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("autostart: mkdir {} failed: {e}", dir.display()))?;
    for id in &list.ids {
        let path = desktop_id_path(id);
        // The "extra" overlay just ensures Hidden=false is present.
        // It's a minimal entry that the XDG autostart spec resolves
        // to "show this entry."
        let text = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={id}\n\
             Hidden=false\n\
             X-MDE-Generated=true\n"
        );
        std::fs::write(&path, text)
            .map_err(|e| anyhow::anyhow!("autostart: write {} failed: {e}", path.display()))?;
    }
    Ok(())
}

/// Read the current autostart list for the matching key. Scans the
/// user's autostart dir, returning every `.desktop` ID we authored
/// (X-MDE-Generated=true) with the matching `Hidden` value.
///
/// # Errors
///
/// Returns an error when the key isn't an autostart key.
pub fn current(key: SettingKey) -> anyhow::Result<SettingValue> {
    let want_hidden = match key {
        SettingKey::AutostartHidden => true,
        SettingKey::AutostartExtra => false,
        _ => anyhow::bail!("autostart: {key} is not an autostart key"),
    };
    let mut ids: Vec<String> = Vec::new();
    let dir = autostart_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !text.contains("X-MDE-Generated=true") {
                continue;
            }
            let is_hidden = text
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case("Hidden=true"));
            if is_hidden != want_hidden {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                ids.push(stem.to_owned());
            }
        }
    }
    ids.sort();
    SettingValue::from_serde(&AutostartList { ids })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn with_xdg<R>(tmp: &std::path::Path, body: impl FnOnce() -> R) -> R {
        let lock = ENV_LOCK.get_or_init(|| Mutex::new(()));
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", tmp);
        let r = body();
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        r
    }

    #[test]
    fn hidden_overlay_text_carries_the_id_and_hidden_flag() {
        let text = hidden_overlay_text("firefox");
        assert!(text.contains("Name=firefox"));
        assert!(text.contains("Hidden=true"));
        assert!(text.contains("X-MDE-Generated=true"));
    }

    #[test]
    fn desktop_id_path_lives_under_autostart_dir() {
        let path = desktop_id_path("firefox");
        assert!(path.to_string_lossy().contains("autostart"));
        assert!(path.to_string_lossy().ends_with("firefox.desktop"));
    }

    #[test]
    fn apply_hidden_then_current_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            let value = SettingValue::from_serde(&AutostartList {
                ids: vec!["firefox".into(), "krita".into()],
            })
            .expect("ser");
            apply(SettingKey::AutostartHidden, &value).expect("apply");
            let got = current(SettingKey::AutostartHidden).expect("current");
            let list: AutostartList = got.to_serde().expect("de");
            assert_eq!(list.ids, vec!["firefox".to_string(), "krita".to_string()]);
        });
    }

    #[test]
    fn apply_extra_then_current_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            let value = SettingValue::from_serde(&AutostartList {
                ids: vec!["mde-panel".into()],
            })
            .expect("ser");
            apply(SettingKey::AutostartExtra, &value).expect("apply");
            let got = current(SettingKey::AutostartExtra).expect("current");
            let list: AutostartList = got.to_serde().expect("de");
            assert_eq!(list.ids, vec!["mde-panel".to_string()]);
        });
    }

    #[test]
    fn apply_hidden_does_not_pick_up_extra_entries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            apply(
                SettingKey::AutostartHidden,
                &SettingValue::from_serde(&AutostartList {
                    ids: vec!["a".into()],
                })
                .unwrap(),
            )
            .unwrap();
            apply(
                SettingKey::AutostartExtra,
                &SettingValue::from_serde(&AutostartList {
                    ids: vec!["b".into()],
                })
                .unwrap(),
            )
            .unwrap();
            let hidden = current(SettingKey::AutostartHidden).unwrap();
            let extra = current(SettingKey::AutostartExtra).unwrap();
            let hidden_list: AutostartList = hidden.to_serde().unwrap();
            let extra_list: AutostartList = extra.to_serde().unwrap();
            assert_eq!(hidden_list.ids, vec!["a".to_string()]);
            assert_eq!(extra_list.ids, vec!["b".to_string()]);
        });
    }

    #[test]
    fn current_skips_desktop_files_not_authored_by_us() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_xdg(tmp.path(), || {
            let autostart = tmp.path().join("autostart");
            std::fs::create_dir_all(&autostart).unwrap();
            // A `.desktop` file without our marker — should be
            // ignored.
            std::fs::write(
                autostart.join("vendor.desktop"),
                "[Desktop Entry]\nType=Application\nName=Vendor\nHidden=true\n",
            )
            .unwrap();
            let got = current(SettingKey::AutostartHidden).unwrap();
            let list: AutostartList = got.to_serde().unwrap();
            assert!(
                list.ids.is_empty(),
                "non-MDE-generated entries must not show up: {:?}",
                list.ids
            );
        });
    }

    #[test]
    fn apply_rejects_non_autostart_key() {
        let value = SettingValue::from_serde(&AutostartList {
            ids: vec!["a".into()],
        })
        .unwrap();
        let r = apply(SettingKey::ThemeName, &value);
        assert!(r.is_err());
    }
}
