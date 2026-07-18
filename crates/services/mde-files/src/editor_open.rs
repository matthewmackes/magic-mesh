//! EDITOR-9 (Part B) — the **Files → Editor** cross-surface open seam.
//!
//! "Send-to-Editor" hands a selected file to the one Construct shell's code-editor
//! surface. Reuse, not reimplementation (§6): it uses the **same persist-first Bus
//! verb pattern** the other Send-To actions use (`chat_bridge`'s `action/chat/send`,
//! `mesh_mount`'s `action/mesh-mount/*`) — a surface writes a typed verb onto a
//! local `Persist`; a consumer drains it. Here the consumer is the shell's editor
//! mount, which reads [`ACTION_EDITOR_OPEN`] and calls
//! `EditorSurface::open_path` (the EDITOR-3 seam).
//!
//! The wire body is [`EditorOpenRequest`] (one file path). The pure builder/parser
//! is unit-tested here; behind the `dbus` feature, [`BusEditorLaunch`] writes the
//! verb and [`EditorLaunchWatch`] drains it (both degrade to an honest no-op when
//! this node has no Bus — never a panic, never a hang).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The verb the shell's editor mount drains: open a file path in `Surface::Editor`.
///
/// A JSON boundary — this crate owns the request shape; the shell parses it with
/// [`EditorOpenRequest::from_body`] and calls its editor's `open_path`.
pub const ACTION_EDITOR_OPEN: &str = "action/editor/open";

/// The `action/editor/open` request body: the file to open in the Editor surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorOpenRequest {
    /// The file to open (absolute — resolved from the selected row's path).
    pub path: PathBuf,
}

impl EditorOpenRequest {
    /// Build a request to open `path`.
    #[must_use]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    /// Serialize to the JSON wire body. An (impossible) serialize failure yields an
    /// empty string — honest, never a panic (`unwrap_used`/`panic` are lint-denied).
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a wire body back into a request; `None` on malformed JSON (the drain
    /// skips it rather than acting on a garbled path).
    #[must_use]
    pub fn from_body(body: &str) -> Option<Self> {
        serde_json::from_str(body).ok()
    }
}

#[cfg(feature = "dbus")]
pub use bus::{BusEditorLaunch, EditorLaunchWatch};

#[cfg(feature = "dbus")]
mod bus {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{EditorOpenRequest, ACTION_EDITOR_OPEN};

    /// The live sender — a synchronous local `Persist` write onto
    /// [`ACTION_EDITOR_OPEN`], the same persist-first path `BusChatBridge` /
    /// `BusMeshMount` take. Holds only the resolved Bus spool dir; a fresh
    /// `Persist` opens per call (it isn't `Send`).
    pub struct BusEditorLaunch {
        /// The resolved Bus client spool dir, or `None` when this node has no Bus.
        bus_root: Option<PathBuf>,
    }

    impl BusEditorLaunch {
        /// Resolve the Bus spool dir from the environment (the production path).
        #[must_use]
        pub fn from_env() -> Self {
            Self {
                bus_root: mde_bus::client_data_dir(),
            }
        }

        /// Construct with an explicit spool root (tests point this at a tempdir, or
        /// `None` to exercise the honest no-Bus no-op).
        #[must_use]
        pub fn with_root(bus_root: Option<PathBuf>) -> Self {
            Self { bus_root }
        }

        /// Post an open request for `path`. Best-effort — a missing Bus / open
        /// failure is a silent no-op, never a panic.
        pub fn send(&self, path: &Path) {
            let Some(root) = self.bus_root.clone() else {
                return; // no Bus on this node — the honest solo-host no-op
            };
            let Ok(persist) = Persist::open(root) else {
                return; // a transient open failure = a silent no-op
            };
            let body = EditorOpenRequest::new(path).to_body();
            let _ = persist.write(ACTION_EDITOR_OPEN, Priority::Default, None, Some(&body));
        }
    }

    /// The cadence for [`EditorLaunchWatch::take`] — the shell calls it every frame,
    /// so the Bus is read at most this often.
    const POLL: Duration = Duration::from_millis(300);

    /// The shell-side drain: reads the newest not-yet-seen [`ACTION_EDITOR_OPEN`]
    /// request (edge-triggered on the ULID cursor, so each request fires once),
    /// cadence-gated so a per-frame call is cheap. Degrades to `None` with no Bus.
    pub struct EditorLaunchWatch {
        /// The resolved Bus client spool dir, or `None` when this node has no Bus.
        bus_root: Option<PathBuf>,
        /// The last request ULID acted on — the `list_since` cursor (edge-trigger).
        last_ulid: Option<String>,
        /// When the Bus was last read, for the [`POLL`] cadence gate.
        last_poll: Option<Instant>,
    }

    impl EditorLaunchWatch {
        /// Resolve the Bus spool dir from the environment (the production path).
        #[must_use]
        pub fn from_env() -> Self {
            Self::with_root(mde_bus::client_data_dir())
        }

        /// Construct with an explicit spool root (tests point this at a tempdir).
        #[must_use]
        pub fn with_root(bus_root: Option<PathBuf>) -> Self {
            Self {
                bus_root,
                last_ulid: None,
                last_poll: None,
            }
        }

        /// The newest unseen open request's path, if one has landed since the last
        /// read. Cadence-gated (returns `None` until [`POLL`] has elapsed), then
        /// drains the Bus edge-triggered. Honest no-Bus / dark-Bus → `None`.
        pub fn take(&mut self) -> Option<PathBuf> {
            let due = self.last_poll.is_none_or(|t| t.elapsed() >= POLL);
            if !due {
                return None;
            }
            self.last_poll = Some(Instant::now());
            self.drain()
        }

        /// Read the newest request past the ULID cursor, advancing it — the core
        /// edge-triggered drain, ignoring the cadence (so tests can exercise it
        /// directly). A malformed body advances the cursor and yields `None` (skip).
        fn drain(&mut self) -> Option<PathBuf> {
            let root = self.bus_root.clone()?;
            let persist = Persist::open(root).ok()?;
            let msgs = persist
                .list_since(ACTION_EDITOR_OPEN, self.last_ulid.as_deref())
                .ok()?;
            let newest = msgs.last()?;
            self.last_ulid = Some(newest.ulid.clone());
            let body = newest.body.as_deref()?;
            EditorOpenRequest::from_body(body).map(|req| req.path)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{BusEditorLaunch, EditorLaunchWatch};
        use std::path::{Path, PathBuf};
        use std::time::{SystemTime, UNIX_EPOCH};

        /// A unique temp dir used as a Bus spool root, cleaned up on drop.
        struct TempDir(PathBuf);
        impl TempDir {
            fn new(tag: &str) -> Self {
                let base = std::env::temp_dir().join(format!(
                    "mde-files-editor-open-{}-{}-{}",
                    tag,
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                std::fs::create_dir_all(&base).expect("create temp bus root");
                Self(base)
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }

        #[test]
        fn send_then_drain_round_trips_the_exact_path() {
            let bus = TempDir::new("rt");
            let file = Path::new("/home/matthew/notes/todo.rs");
            BusEditorLaunch::with_root(Some(bus.0.clone())).send(file);

            let mut watch = EditorLaunchWatch::with_root(Some(bus.0.clone()));
            assert_eq!(
                watch.take(),
                Some(file.to_path_buf()),
                "the drained request carries the exact posted path"
            );
        }

        #[test]
        fn drain_is_edge_triggered_and_fires_once_per_request() {
            let bus = TempDir::new("edge");
            let sender = BusEditorLaunch::with_root(Some(bus.0.clone()));
            let mut watch = EditorLaunchWatch::with_root(Some(bus.0.clone()));

            sender.send(Path::new("/tmp/one.txt"));
            assert_eq!(watch.drain(), Some(PathBuf::from("/tmp/one.txt")));
            // Already consumed — the ULID cursor advanced, so it does not re-fire.
            assert_eq!(watch.drain(), None, "a consumed request fires only once");

            // A second request past the cursor is picked up.
            sender.send(Path::new("/tmp/two.txt"));
            assert_eq!(watch.drain(), Some(PathBuf::from("/tmp/two.txt")));
        }

        #[test]
        fn no_bus_root_is_a_silent_no_op() {
            // The honest solo-host path: no Bus dir → send does nothing, drain None.
            BusEditorLaunch::with_root(None).send(Path::new("/tmp/whatever.rs"));
            let mut watch = EditorLaunchWatch::with_root(None);
            assert_eq!(watch.take(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorOpenRequest, ACTION_EDITOR_OPEN};
    use std::path::PathBuf;

    #[test]
    fn the_verb_is_the_action_editor_open_topic() {
        assert_eq!(ACTION_EDITOR_OPEN, "action/editor/open");
    }

    #[test]
    fn body_carries_and_round_trips_the_path() {
        let req = EditorOpenRequest::new("/home/matthew/src/lib.rs");
        let body = req.to_body();
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(value["path"], "/home/matthew/src/lib.rs");

        let back = EditorOpenRequest::from_body(&body).expect("parses back");
        assert_eq!(back.path, PathBuf::from("/home/matthew/src/lib.rs"));
        assert_eq!(back, req);
    }

    #[test]
    fn malformed_body_parses_to_none() {
        assert!(EditorOpenRequest::from_body("not json").is_none());
        assert!(EditorOpenRequest::from_body("{}").is_none());
    }
}
