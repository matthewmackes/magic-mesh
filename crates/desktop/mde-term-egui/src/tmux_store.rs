//! TMUX-FC-5 — the **platform-managed persisted tmux state**: the remembered
//! session (auto-reattached on relaunch) + the saved templates ("projects").
//!
//! Design: `docs/design/tmux-first-class.md` (#5 sessions/templates, #6
//! persistence). Persisted under the platform config dir
//! ([`mde_bus::client_data_dir`] — the one data root every mesh surface already
//! resolves) as `<root>/tmux/state.json`, written with the **atomic
//! temp+rename** idiom (the `mackesd`/[`crate::layout`] discipline) so a reader
//! never sees a half-written file and a crash mid-save never corrupts the state.
//!
//! This is deliberately **node-local** (not the mesh-synced workgroup root a
//! saved *layout* uses): "which session this client reattaches to" is a property
//! of *this* terminal on *this* node, not a shared artefact. The mesh-synced
//! surface is the Construct tmux **config** (TMUX-FC-8), a separate store.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::blueprint::Blueprint;

/// The config subdirectory the state file lives under (sibling of the Bus spool).
const TMUX_SUBDIR: &str = "tmux";
/// The state file name.
const STATE_FILE: &str = "state.json";
/// Keep the persisted terminal state bounded before `serde_json` materializes
/// user-controlled templates and blueprints.
const MAX_TMUX_STATE_BYTES: usize = 256 * 1024;

/// A saved session template — a user-named "project" that seeds a whole
/// layout + commands ([`Blueprint`]) when opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTemplate {
    /// The template name (its launch-menu label; also the tmux session name it
    /// opens, sanitised through [`session_safe`]).
    pub name: String,
    /// The layout + seeded commands the template builds.
    pub blueprint: Blueprint,
}

impl SessionTemplate {
    /// A template named `name` building `blueprint`.
    #[must_use]
    pub fn new(name: impl Into<String>, blueprint: Blueprint) -> Self {
        Self {
            name: name.into(),
            blueprint,
        }
    }
}

/// The whole persisted tmux state for this terminal on this node.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxState {
    /// The session name this terminal was last attached to — auto-reattached on
    /// relaunch (a still-running detached session is re-entered; a killed one is
    /// recreated, `new-session -A` semantics). `None` when the user never opted
    /// into tmux, so a terminal that never touched it stays quiet on relaunch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session: Option<String>,
    /// The user's saved templates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<SessionTemplate>,
}

/// The persistence handle: reads + atomically writes [`TmuxState`].
///
/// Under the platform config dir. A `None` path (no Bus/data dir resolved)
/// degrades honestly — loads empty, refuses to save with a clear error (never a
/// silent drop onto nowhere).
#[derive(Clone, Debug, Default)]
pub struct TmuxStateStore {
    /// The resolved `<root>/tmux/state.json` path, or `None` when no config dir
    /// could be resolved.
    path: Option<PathBuf>,
}

impl TmuxStateStore {
    /// The production store — the state file under the resolved platform config
    /// dir ([`mde_bus::client_data_dir`]).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            path: mde_bus::client_data_dir().map(|d| d.join(TMUX_SUBDIR).join(STATE_FILE)),
        }
    }

    /// A store over an explicit state-file path (the test seam).
    #[must_use]
    pub const fn with_path(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    /// Load the persisted state — an honest [`TmuxState::default`] when the file
    /// is absent, unreadable, or malformed (a half-write / a hand-edit never
    /// breaks the surface's boot).
    #[must_use]
    pub fn load(&self) -> TmuxState {
        let Some(path) = self.path.as_ref() else {
            return TmuxState::default();
        };
        read_bounded_state(path)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persist `state` atomically (temp + rename), creating the config subdir.
    ///
    /// # Errors
    /// [`io::ErrorKind::NotFound`] when no config dir resolved (nothing to write
    /// to — surfaced, never faked), or the underlying write/rename error.
    pub fn save(&self, state: &TmuxState) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no platform config dir — tmux state not saved",
            ));
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Read the persisted state through the descriptor that is consumed.
///
/// The state file is normally written atomically by [`TmuxStateStore::save`],
/// but it still crosses a filesystem boundary that can be replaced or made
/// hostile. Refuse final symlinks, special files, oversized or changing input,
/// and invalid UTF-8 before handing the body to `serde_json`.
fn read_bounded_state(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000 | 0o4000); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_file())
        {
            return None;
        }
    }
    #[cfg(not(unix))]
    if !fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file())
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.file_type().is_file() || before.len() > MAX_TMUX_STATE_BYTES as u64 {
        return None;
    }

    let capacity = usize::try_from(before.len())
        .unwrap_or(MAX_TMUX_STATE_BYTES)
        .min(MAX_TMUX_STATE_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    (&file)
        .take((MAX_TMUX_STATE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != before.len()
        || bytes.len() > MAX_TMUX_STATE_BYTES
        || bytes.len() as u64 != before.len()
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Sanitise a name into a tmux session name.
///
/// tmux forbids `.` and `:` in a session name (they address windows/panes), so
/// both collapse to `-`; a trimmed-empty result falls back to `session` so a
/// session always has a name.
#[must_use]
pub fn session_safe(name: &str) -> String {
    let mapped: String = name
        .trim()
        .chars()
        .map(|c| if c == '.' || c == ':' { '-' } else { c })
        .collect();
    let mapped = mapped.trim().trim_matches('-').trim();
    if mapped.is_empty() {
        "session".to_owned()
    } else {
        mapped.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{BlueprintPane, BlueprintWindow};
    use crate::splits::SplitDir;
    use crate::tmux::StockLayout;

    fn sample_template() -> SessionTemplate {
        SessionTemplate::new(
            "Dev",
            Blueprint::new(vec![BlueprintWindow::new(
                "edit",
                vec![BlueprintPane::cmd("vim"), BlueprintPane::shell()],
                SplitDir::V,
                Some(StockLayout::Tiled),
            )]),
        )
    }

    #[test]
    fn state_round_trips_through_the_atomic_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tmux").join("state.json");
        let store = TmuxStateStore::with_path(Some(path));
        let state = TmuxState {
            last_session: Some("main".to_owned()),
            templates: vec![sample_template()],
        };
        store.save(&state).expect("save");
        assert_eq!(store.load(), state, "the saved state reads back identical");
    }

    #[test]
    fn a_missing_or_malformed_file_loads_the_empty_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tmux").join("state.json");
        let store = TmuxStateStore::with_path(Some(path.clone()));
        // Missing → default.
        assert_eq!(store.load(), TmuxState::default());
        // Malformed → default (a hand-edit / half-write never wedges boot).
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{ not json").expect("write junk");
        assert_eq!(store.load(), TmuxState::default());
    }

    #[test]
    fn bounded_state_reader_rejects_hostile_inputs_before_deserializing() {
        let dir = tempfile::tempdir().expect("state tempdir");
        let path = dir.path().join("state.json");
        let store = TmuxStateStore::with_path(Some(path.clone()));

        fs::write(&path, b"{}").expect("write valid state");
        assert_eq!(read_bounded_state(&path).as_deref(), Some("{}"));

        fs::write(&path, [0xff, 0xfe]).expect("write invalid state");
        assert!(read_bounded_state(&path).is_none());
        assert_eq!(store.load(), TmuxState::default());

        fs::write(&path, vec![b'{'; MAX_TMUX_STATE_BYTES + 1]).expect("write oversized state");
        assert!(read_bounded_state(&path).is_none());
        assert_eq!(store.load(), TmuxState::default());

        fs::remove_file(&path).expect("remove regular state");
        fs::create_dir(&path).expect("create non-regular state");
        assert!(read_bounded_state(&path).is_none());
        assert_eq!(store.load(), TmuxState::default());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            use std::os::unix::net::UnixListener;

            fs::remove_dir(&path).expect("remove non-regular state");
            let socket = dir.path().join("state.sock");
            let _listener = UnixListener::bind(&socket).expect("create socket state");
            assert!(read_bounded_state(&socket).is_none());

            let target = dir.path().join("outside.json");
            fs::write(&target, b"{}").expect("write symlink target");
            symlink(&target, &path).expect("create final symlink");
            assert!(read_bounded_state(&path).is_none());
            assert_eq!(store.load(), TmuxState::default());
        }
    }

    #[test]
    fn a_pathless_store_loads_empty_and_refuses_to_save() {
        let store = TmuxStateStore::with_path(None);
        assert_eq!(store.load(), TmuxState::default());
        let err = store
            .save(&TmuxState::default())
            .expect_err("no path → error");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn session_safe_strips_tmux_reserved_chars_and_never_empties() {
        assert_eq!(session_safe("Dev"), "Dev");
        assert_eq!(session_safe("web:8080"), "web-8080");
        assert_eq!(session_safe("a.b.c"), "a-b-c");
        assert_eq!(session_safe("  spaced  "), "spaced");
        assert_eq!(session_safe("..."), "session");
        assert_eq!(session_safe(""), "session");
    }
}
