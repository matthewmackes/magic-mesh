//! **EDITOR-11 autosave preference (persisted)** — the [`AutosavePrefs`] model
//! plus its config-path / read / write / load helpers, split out of the `panel`
//! god-module (pure relocation, no behaviour change). The parent surface owns an
//! `AutosavePrefs` field and drives the debounced write; this leaf reads the
//! parent's private imports (serde, `io`, `Path`/`PathBuf`) via `use super::*`.

use super::*;

/// The default autosave idle window in seconds — the debounce so a mid-keystroke
/// burst never triggers a write; a dirty buffer is saved only after it has been
/// idle this long.
const DEFAULT_AUTOSAVE_SECS: f64 = 2.0;

/// The editor's config file basename under `<config>/mcnf/` (EDITOR-11). Only the
/// production config-path resolver reads it; the suite is cfg-gated to never touch
/// the real user config (see [`autosave_config_path`]).
#[cfg(not(test))]
const AUTOSAVE_CONFIG_FILE: &str = "editor-egui.json";

/// The persisted editor preferences (EDITOR-11) — currently the autosave toggle
/// and its debounce window. Serialized as `<config>/mcnf/editor-egui.json`. Off
/// by default (the acceptance): a fresh install never autosaves until the
/// operator opts in.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct AutosavePrefs {
    /// Whether debounced autosave is on. Off by default.
    #[serde(default)]
    pub(super) enabled: bool,
    /// Idle seconds before a dirty buffer is written (the debounce window).
    #[serde(default = "default_autosave_secs")]
    pub(super) idle_secs: f64,
}

/// The serde default for [`AutosavePrefs::idle_secs`] (a bare `#[serde(default)]`
/// would zero it, defeating the debounce).
const fn default_autosave_secs() -> f64 {
    DEFAULT_AUTOSAVE_SECS
}

impl Default for AutosavePrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_secs: DEFAULT_AUTOSAVE_SECS,
        }
    }
}

/// The editor config file path under the user config dir, or `None` when neither
/// `XDG_CONFIG_HOME` nor `HOME` resolves. Under `cfg(test)` this is always `None`
/// so the suite never reads or writes the real user config (the round-trip is
/// proven against an explicit tempdir instead — the same cfg-gated seam
/// `build_lsp_client` uses to keep the suite hermetic).
#[cfg(not(test))]
pub(super) fn autosave_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mcnf").join(AUTOSAVE_CONFIG_FILE))
}

#[cfg(test)]
pub(super) const fn autosave_config_path() -> Option<PathBuf> {
    None
}

/// Read the persisted prefs at `path`, clamping the idle window to a sane floor
/// (a hand-edited `0`/negative would thrash) — the shared reader for both the
/// production load and the round-trip test.
pub(super) fn read_autosave_prefs_at(path: &Path) -> Option<AutosavePrefs> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut prefs: AutosavePrefs = serde_json::from_str(&data).ok()?;
    prefs.idle_secs = prefs.idle_secs.max(0.2);
    Some(prefs)
}

/// Write `prefs` to `path` as pretty JSON, creating the parent directory — the
/// shared writer for both the production save and the round-trip test.
///
/// # Errors
/// Returns an [`io::Error`] if the directory cannot be created or the write fails.
pub(super) fn write_autosave_prefs_at(path: &Path, prefs: AutosavePrefs) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&prefs)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

/// The persisted prefs from the resolved config path, or the default (off) when
/// nothing is stored / the path does not resolve (§7 — an honest off default,
/// never a fabricated toggle).
pub(super) fn load_autosave_prefs() -> AutosavePrefs {
    autosave_config_path()
        .as_deref()
        .and_then(read_autosave_prefs_at)
        .unwrap_or_default()
}
