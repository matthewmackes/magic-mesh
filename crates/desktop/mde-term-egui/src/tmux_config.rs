//! TMUX-FC-8 — the **platform-managed Quasar tmux config** (mesh-synced) + its
//! application over the control channel.
//!
//! Design: `docs/design/tmux-first-class.md` (#14 config: a Quasar default,
//! mesh-synced, no per-user file hand-editing). A small model — the prefix, the
//! mouse toggle, the scrollback history limit — that the operator edits in a GUI
//! pane rather than by hand-editing a `.tmux.conf`.
//!
//! **Mesh-synced** ([`TmuxConfigStore`]): a single shared JSON under the
//! Syncthing-replicated workgroup root (`<root>/tmux/config.json`, resolved
//! through `mackes_mesh_types::peers::default_workgroup_root` — the one mount the
//! saved-layout store and every mesh surface already share), so a config saved on
//! one node reaches every node once Syncthing replicates it. Last-writer-wins
//! (the ddns/exposure-config idiom), acceptable for a rarely-edited operator
//! artefact.
//!
//! **Applied over control mode** ([`TmuxConfig::option_commands`]): rather than a
//! per-user file a fresh server may or may not have read, the config is pushed as
//! `set-option -g` commands the moment the control client attaches — so the live
//! server always reflects the Quasar config regardless of what `.tmux.conf` (if
//! any) it started from. [`TmuxConfig::to_conf`] still renders the equivalent
//! `.tmux.conf` text (the canonical artefact + a test anchor).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The share subdirectory + file the mesh-synced config lives at.
const TMUX_SUBDIR: &str = "tmux";
/// The config file name under [`TMUX_SUBDIR`].
const CONFIG_FILE: &str = "config.json";

/// The canonical deployed shared-storage mount (mirrors [`crate::layout`]'s
/// guard): writable only once it actually exists, so a save before the mesh
/// share is provisioned fails honestly rather than landing on a bare local dir.
const CANONICAL_MOUNT: &str = "/mnt/mesh-storage";

/// The default scrollback history limit (lines) — the Quasar default, matching a
/// generous-but-bounded terminal.
const DEFAULT_HISTORY: u32 = 50_000;

/// The platform-managed Quasar tmux config: the operator-tunable knobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxConfig {
    /// The tmux prefix key token (tmux's own syntax, e.g. `C-b`, `C-a`,
    /// `C-Space`). Sent as `set-option -g prefix <prefix>`; the prefix still
    /// passes through to panes (design lock #12) — the native GUI chords are a
    /// separate, remappable keymap, not this.
    pub prefix: String,
    /// Whether tmux mouse support is on (`set -g mouse on/off`).
    pub mouse: bool,
    /// The scrollback history limit in lines (`set -g history-limit <n>`).
    pub history_limit: u32,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            prefix: "C-b".to_owned(),
            mouse: true,
            history_limit: DEFAULT_HISTORY,
        }
    }
}

impl TmuxConfig {
    /// The tmux prefix token, sanitised to a safe key spec (tmux key tokens are
    /// alphanumerics, `-`, and `_`; anything else is dropped). Falls back to the
    /// default `C-b` when the result is empty, so the emitted command is always
    /// well-formed.
    #[must_use]
    pub fn safe_prefix(&self) -> String {
        let cleaned: String = self
            .prefix
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if cleaned.is_empty() {
            "C-b".to_owned()
        } else {
            cleaned
        }
    }

    /// The `set-option`/`set` command lines that apply this config over the
    /// control channel (design #14 — no file to hand-edit): the prefix, the mouse
    /// toggle, and the history limit. `send-prefix` on a double-press keeps the
    /// prefix usable inside a nested program (design lock #12 pass-through).
    #[must_use]
    pub fn option_commands(&self) -> Vec<String> {
        let prefix = self.safe_prefix();
        vec![
            format!("set-option -g prefix {prefix}"),
            format!("bind-key {prefix} send-prefix"),
            format!("set-option -g mouse {}", on_off(self.mouse)),
            format!("set-option -g history-limit {}", self.history_limit),
        ]
    }

    /// The equivalent `.tmux.conf` text — the canonical Quasar config artefact
    /// (what the operator would see as a file), kept in step with
    /// [`Self::option_commands`].
    #[must_use]
    pub fn to_conf(&self) -> String {
        let prefix = self.safe_prefix();
        format!(
            "# Quasar tmux config (platform-managed, mesh-synced) — TMUX-FC-8\n\
             set-option -g prefix {prefix}\n\
             bind-key {prefix} send-prefix\n\
             set-option -g mouse {}\n\
             set-option -g history-limit {}\n",
            on_off(self.mouse),
            self.history_limit,
        )
    }
}

/// `on`/`off` for a tmux boolean option.
const fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// The mesh-synced config store: reads + atomically writes the one shared
/// [`TmuxConfig`] under the workgroup root.
#[derive(Clone, Debug)]
pub struct TmuxConfigStore {
    /// The Syncthing-replicated workgroup root.
    root: PathBuf,
}

impl TmuxConfigStore {
    /// A store over an explicit root (the test seam).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The production store — the shared workgroup root every mesh surface
    /// resolves through the one canonical helper.
    #[must_use]
    pub fn local() -> Self {
        Self::new(mackes_mesh_types::peers::default_workgroup_root())
    }

    /// The `<root>/tmux/config.json` path.
    fn path(&self) -> PathBuf {
        self.root.join(TMUX_SUBDIR).join(CONFIG_FILE)
    }

    /// Load the mesh config — the Quasar [`TmuxConfig::default`] when absent,
    /// unreadable, or malformed (a half-write / a fresh mesh never wedges boot).
    #[must_use]
    pub fn load(&self) -> TmuxConfig {
        fs::read_to_string(self.path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persist `config` atomically (temp + rename) under the workgroup root, so a
    /// reader — local or a peer after replication — never sees a half-written
    /// file.
    ///
    /// # Errors
    /// An honest [`io::Error`] when the shared root is not yet provisioned (a bare
    /// canonical mount) or the write/rename fails — never a faked success.
    pub fn save(&self, config: &TmuxConfig) -> io::Result<PathBuf> {
        if !root_writable(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("mesh share {CANONICAL_MOUNT} is not mounted yet — tmux config not saved"),
            ));
        }
        let path = self.path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

/// Whether it is safe to write under `root` (the canonical mount only once it
/// exists; any dev/test root always writable) — the [`crate::layout`] guard.
fn root_writable(root: &Path) -> bool {
    root != Path::new(CANONICAL_MOUNT) || root.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_commands_apply_the_prefix_mouse_and_history() {
        let cfg = TmuxConfig {
            prefix: "C-a".to_owned(),
            mouse: true,
            history_limit: 20_000,
        };
        assert_eq!(
            cfg.option_commands(),
            vec![
                "set-option -g prefix C-a".to_owned(),
                "bind-key C-a send-prefix".to_owned(),
                "set-option -g mouse on".to_owned(),
                "set-option -g history-limit 20000".to_owned(),
            ]
        );
        // Mouse off flips only the mouse line.
        let off = TmuxConfig {
            mouse: false,
            ..cfg
        };
        assert!(off
            .option_commands()
            .contains(&"set-option -g mouse off".to_owned()));
    }

    #[test]
    fn the_default_is_the_quasar_config() {
        let cfg = TmuxConfig::default();
        assert_eq!(cfg.prefix, "C-b");
        assert!(cfg.mouse);
        assert_eq!(cfg.history_limit, DEFAULT_HISTORY);
    }

    #[test]
    fn a_dangerous_prefix_is_sanitised_before_it_reaches_a_command() {
        // A prefix carrying tmux command metacharacters can't inject a second
        // command — only key-token characters survive.
        let cfg = TmuxConfig {
            prefix: "C-a; kill-server".to_owned(),
            ..TmuxConfig::default()
        };
        assert_eq!(cfg.safe_prefix(), "C-akill-server");
        assert!(cfg
            .option_commands()
            .iter()
            .all(|c| !c.contains("kill-server;") && !c.contains(';')));
        // An all-punctuation prefix falls back to the default.
        let empty = TmuxConfig {
            prefix: "!!!".to_owned(),
            ..TmuxConfig::default()
        };
        assert_eq!(empty.safe_prefix(), "C-b");
    }

    #[test]
    fn to_conf_renders_the_equivalent_file() {
        let conf = TmuxConfig::default().to_conf();
        assert!(conf.contains("set-option -g prefix C-b"));
        assert!(conf.contains("set-option -g mouse on"));
        assert!(conf.contains("set-option -g history-limit 50000"));
    }

    #[test]
    fn config_round_trips_through_the_mesh_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TmuxConfigStore::new(dir.path());
        let cfg = TmuxConfig {
            prefix: "C-Space".to_owned(),
            mouse: false,
            history_limit: 100_000,
        };
        store.save(&cfg).expect("save");
        // safe_prefix keeps `C-Space` minus the space… but the stored model keeps
        // the raw prefix; only the emitted command sanitises. Assert the model.
        let loaded = store.load();
        assert_eq!(loaded, cfg, "the saved config reads back identical");
    }

    #[test]
    fn a_config_saved_on_one_node_is_visible_on_another() {
        // Two stores over the SAME replicated root (what Syncthing gives peers).
        let dir = tempfile::tempdir().expect("tempdir");
        let node_a = TmuxConfigStore::new(dir.path());
        let node_b = TmuxConfigStore::new(dir.path());
        let cfg = TmuxConfig {
            prefix: "C-a".to_owned(),
            ..TmuxConfig::default()
        };
        node_a.save(&cfg).expect("save on A");
        assert_eq!(node_b.load(), cfg, "node B sees node A's config");
    }

    #[test]
    fn a_missing_config_loads_the_quasar_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TmuxConfigStore::new(dir.path().join("never-synced"));
        assert_eq!(store.load(), TmuxConfig::default());
    }
}
