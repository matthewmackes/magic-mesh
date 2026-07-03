//! TERM-10 — saved terminal **layouts**, mesh-synced.
//!
//! A [`SavedLayout`] is the serializable recipe of a whole surface arrangement:
//! every tab's Terminator split tree ([`LayoutTab`] / [`LayoutPane`]) plus, per
//! pane, the launch spec ([`PaneSpec`] — a local pane's cwd + command, or a
//! remote pane's target node). It is the **projection** of the live surface the
//! model can persist: the runtime [`crate::splits::Pane`] tree keys its leaves by
//! a [`crate::splits::SessionId`] (a per-process registry handle, meaningless
//! across launches), so a saved layout mirrors that exact tree *shape* — reusing
//! the surface's [`crate::splits::SplitDir`] and its [`crate::RemoteTarget`] — but
//! carries the pane's launch content in place of the runtime id. [`SplitTerminal`]
//! (`splits.rs`) captures a live tree into a [`LayoutTab`] and rebuilds one from
//! a [`LayoutTab`]; this module is the pure model + the synced store.
//!
//! [`SplitTerminal`]: crate::splits::SplitTerminal
//!
//! **Mesh sync (§6 — reuse, no new mechanism).** [`LayoutStore`] persists layouts
//! exactly the way the `mackesd` bookmarks worker persists its op segments: a
//! single-writer-per-node directory under the Syncthing-replicated workgroup root
//! (`<root>/terminal-layouts/<node>/<slug>.json`, resolved through
//! `mackes_mesh_types::peers::default_workgroup_root` — the one mount every mesh
//! surface shares). Each node writes only into its own directory, so Syncthing
//! never sees a write conflict; a reader unions every node's directory, so a
//! layout saved on one node is visible + launchable on another the moment
//! Syncthing has replicated the file. The daemon does the replication out of band
//! — this surface only writes and reads plain JSON under the shared dir.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::picker::RemoteTarget;
use crate::splits::SplitDir;

/// The share subdirectory saved layouts live under (sibling of the bookmarks
/// worker's `bookmarks/` tree, same single-writer-per-node discipline).
pub const LAYOUTS_SUBDIR: &str = "terminal-layouts";

/// The canonical deployed shared-storage mount — mirrors `mackesd`'s
/// `CANONICAL_QNM_MOUNT`. Under SUBSTRATE-V2 it is a plain Syncthing-replicated
/// directory, writable only once it actually exists: writing before the first
/// sync provisions it would land a layout on a bare, unreplicated local dir (the
/// exact split-brain the bookmarks worker guards against).
const CANONICAL_MOUNT: &str = "/mnt/mesh-storage";

/// One saved, named terminal arrangement — the whole surface as a launchable
/// recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLayout {
    /// The user-facing name (the launch menu label; authoritative over the
    /// on-disk filename slug).
    pub name: String,
    /// The node the layout was saved on — provenance for the launch menu ("saved
    /// on oak"). The file itself lives under this node's store directory.
    pub origin: String,
    /// Every tab's split tree, in strip order.
    pub tabs: Vec<LayoutTab>,
    /// The tab focused on launch (clamped into range when rebuilt).
    #[serde(default)]
    pub active: usize,
}

/// One tab of a saved layout: its strip title and its split tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutTab {
    /// The tab's strip label at save time.
    pub title: String,
    /// The tab's split tree.
    pub root: LayoutPane,
}

/// The serializable projection of the runtime [`crate::splits::Pane`] tree: the
/// same `Leaf | Split { dir, ratio, a, b }` shape (reusing the surface's
/// [`SplitDir`]), but a leaf carries a [`PaneSpec`] launch recipe instead of a
/// runtime [`crate::splits::SessionId`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayoutPane {
    /// One terminal pane, described by how to relaunch it.
    Leaf(PaneSpec),
    /// A rectangle cut in two, to any depth (mirrors [`crate::splits::Pane::Split`]).
    Split {
        /// Which way the cut runs.
        dir: SplitDir,
        /// Child `a`'s share of the rectangle.
        ratio: f32,
        /// The top (`H`) / left (`V`) child.
        a: Box<LayoutPane>,
        /// The bottom (`H`) / right (`V`) child.
        b: Box<LayoutPane>,
    },
}

impl LayoutPane {
    /// A single-pane tree.
    #[must_use]
    pub fn leaf(spec: PaneSpec) -> Self {
        Self::Leaf(spec)
    }

    /// The number of panes (leaves) in the tree.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { a, b, .. } => a.pane_count() + b.pane_count(),
        }
    }
}

/// How to relaunch one pane. A remote pane records its target node (and
/// reconnects over the TERM-7 broker on launch); a local pane records its
/// working directory + launch command (a fresh login shell there on launch).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneSpec {
    /// The mesh node to reconnect to — `Some` for a remote pane, `None` for a
    /// local one. Reuses the surface's [`RemoteTarget`], so the launch path feeds
    /// it straight back into the same remote-open code that opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<RemoteTarget>,
    /// A local pane's working directory at save time (`None` inherits the
    /// process cwd on launch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// A local pane's launch command / shell program (`None` relaunches the
    /// default login shell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl PaneSpec {
    /// A local pane spec — a shell at `cwd` running `command` (either optional).
    #[must_use]
    pub fn local(cwd: Option<PathBuf>, command: Option<String>) -> Self {
        Self {
            target: None,
            cwd,
            command,
        }
    }

    /// A remote pane spec — reconnect to `target` over the broker.
    #[must_use]
    pub fn remote(target: RemoteTarget) -> Self {
        Self {
            target: Some(target),
            cwd: None,
            command: None,
        }
    }

    /// Whether this pane reconnects to a mesh node on launch.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        self.target.is_some()
    }
}

// ── The synced store ─────────────────────────────────────────────────────────

/// The mesh-synced layout store: reads + writes named layouts under the
/// Syncthing-replicated workgroup root, single-writer-per-node.
///
/// The `node` names *this* node's write directory; reads union every node's
/// directory, so a peer's layouts appear here as soon as Syncthing has carried
/// their files across (the two-node visibility the design wants).
pub struct LayoutStore {
    /// The Syncthing-replicated workgroup root (`default_workgroup_root()` in
    /// production; a tempdir in tests).
    root: PathBuf,
    /// This node's name — its single-writer subdirectory.
    node: String,
}

impl LayoutStore {
    /// A store over an explicit `root` writing as `node` (the test seam + the
    /// building block [`Self::local`] resolves the production values into).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, node: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            node: node.into(),
        }
    }

    /// The production store: the shared workgroup root (single-sourced through
    /// `mackes_mesh_types`, byte-for-byte the same mount every mesh surface
    /// resolves) writing under this node's hostname.
    #[must_use]
    pub fn local() -> Self {
        Self::new(
            mackes_mesh_types::peers::default_workgroup_root(),
            local_node(),
        )
    }

    /// This node's write directory (`<root>/terminal-layouts/<node>/`).
    #[must_use]
    pub fn node_dir(&self) -> PathBuf {
        self.layouts_dir().join(&self.node)
    }

    /// The `<root>/terminal-layouts/` directory holding every node's subdir.
    fn layouts_dir(&self) -> PathBuf {
        self.root.join(LAYOUTS_SUBDIR)
    }

    /// Persist `layout` into this node's directory as `<slug>.json`, written
    /// atomically (temp + rename) so a reader — local or, after replication, a
    /// peer — never sees a half-written file. Overwrites a same-named layout.
    ///
    /// # Errors
    /// An honest [`io::Error`] when the shared root is not yet provisioned (a
    /// bare, unmounted canonical mount) or the write / rename fails — never a
    /// faked success onto a non-replicated dir.
    pub fn save(&self, layout: &SavedLayout) -> io::Result<PathBuf> {
        if !root_writable(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("mesh share {CANONICAL_MOUNT} is not mounted yet — layout not saved"),
            ));
        }
        let dir = self.node_dir();
        fs::create_dir_all(&dir)?;
        let slug = slugify(&layout.name);
        let final_path = dir.join(format!("{slug}.json"));
        let tmp_path = dir.join(format!(".{slug}.json.tmp"));
        let json = serde_json::to_string_pretty(layout)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(final_path)
    }

    /// Every saved layout visible from this node — the union of every node's
    /// directory (so peers' layouts show up once replicated), sorted by name
    /// then origin. Malformed / half-written / unreadable files are skipped, not
    /// fatal; a missing store is simply empty.
    #[must_use]
    pub fn list(&self) -> Vec<SavedLayout> {
        let mut out = Vec::new();
        let Ok(nodes) = fs::read_dir(self.layouts_dir()) else {
            return out;
        };
        for node in nodes.flatten() {
            if !node.path().is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(node.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "json") {
                    if let Some(layout) = read_layout(&path) {
                        out.push(layout);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.origin.cmp(&b.origin)));
        out
    }

    /// The first layout named `name` visible from this node, if any.
    #[must_use]
    pub fn load(&self, name: &str) -> Option<SavedLayout> {
        self.list().into_iter().find(|l| l.name == name)
    }
}

/// Read + parse one layout file, returning `None` for anything unreadable or
/// malformed (a concurrent half-write must not break a listing).
fn read_layout(path: &Path) -> Option<SavedLayout> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Whether it is safe to write under `root`. The canonical mount is writable
/// only once it exists (see [`CANONICAL_MOUNT`]); any other root (a dev
/// `~/QNM-Shared`, a tempdir) is always writable.
fn root_writable(root: &Path) -> bool {
    root != Path::new(CANONICAL_MOUNT) || root.is_dir()
}

/// A filesystem-safe slug for a layout name: lowercased, each run of
/// non-alphanumeric characters collapsed to a single `-`, trimmed. Empty names
/// (or all-punctuation ones) fall back to `layout`, so a file always has a name.
/// The layout's `name` field stays authoritative for display — the slug is only
/// the filename.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "layout".to_string()
    } else {
        slug.to_string()
    }
}

/// The current working directory of a live child shell, read from
/// `/proc/<pid>/cwd`. Best-effort: `None` when the pid is gone or the link is
/// unreadable (a remote pane, a race, a non-Linux host) — the pane simply
/// relaunches in the inherited cwd.
#[must_use]
pub fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// This node's name for the store's write directory: `$HOSTNAME` →
/// `/proc/sys/kernel/hostname` → `/etc/hostname` → `"node"` — the same fallback
/// chain the shell + panel surfaces stamp their local peer name from.
#[must_use]
pub fn local_node() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "node".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-tab layout: tab 1 is a vertical split of a local pane (with a cwd +
    /// command) beside a remote pane; tab 2 is a lone local pane. Exercises every
    /// model shape the surface can save.
    fn sample_layout(origin: &str) -> SavedLayout {
        let local = PaneSpec::local(Some(PathBuf::from("/tmp/work")), Some("/bin/bash".into()));
        let remote = PaneSpec::remote(RemoteTarget {
            peer: "oak".into(),
            label: "oak".into(),
        });
        SavedLayout {
            name: "Dev setup".into(),
            origin: origin.into(),
            tabs: vec![
                LayoutTab {
                    title: "1".into(),
                    root: LayoutPane::Split {
                        dir: SplitDir::V,
                        ratio: 0.5,
                        a: Box::new(LayoutPane::leaf(local)),
                        b: Box::new(LayoutPane::leaf(remote)),
                    },
                },
                LayoutTab {
                    title: "2".into(),
                    root: LayoutPane::leaf(PaneSpec::local(None, None)),
                },
            ],
            active: 1,
        }
    }

    #[test]
    fn a_layout_serializes_and_deserializes_to_an_identical_tree() {
        let layout = sample_layout("here");
        let json = serde_json::to_string_pretty(&layout).expect("serialize");
        let back: SavedLayout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(layout, back, "serde round-trip preserves the whole layout");
        // The shape is intact: tab 1 is a 2-pane split, tab 2 a lone pane.
        assert_eq!(back.tabs[0].root.pane_count(), 2);
        assert_eq!(back.tabs[1].root.pane_count(), 1);
        // The remote leaf still carries its target node…
        let LayoutPane::Split { b, .. } = &back.tabs[0].root else {
            panic!("tab 1 is a split");
        };
        let LayoutPane::Leaf(spec) = b.as_ref() else {
            panic!("split's b child is a leaf");
        };
        assert!(spec.is_remote());
        assert_eq!(spec.target.as_ref().expect("target").peer, "oak");
    }

    #[test]
    fn a_local_pane_spec_keeps_its_cwd_and_command_and_a_remote_one_its_target() {
        let local = PaneSpec::local(Some(PathBuf::from("/srv")), Some("zsh".into()));
        assert!(!local.is_remote());
        assert_eq!(local.cwd, Some(PathBuf::from("/srv")));
        assert_eq!(local.command.as_deref(), Some("zsh"));

        let remote = PaneSpec::remote(RemoteTarget {
            peer: "cedar".into(),
            label: "cedar".into(),
        });
        assert!(remote.is_remote());
        assert!(remote.cwd.is_none() && remote.command.is_none());
    }

    #[test]
    fn save_then_list_round_trips_through_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LayoutStore::new(dir.path(), "nodeA");
        let layout = sample_layout("nodeA");
        store.save(&layout).expect("save");

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], layout, "the saved layout reads back identical");
        assert_eq!(store.load("Dev setup").as_ref(), Some(&layout));
        assert!(store.load("missing").is_none());
    }

    #[test]
    fn a_layout_saved_on_one_node_is_visible_and_identical_on_another() {
        // Both stores point at the SAME replicated root (what Syncthing gives two
        // nodes) but write under their own node directory.
        let dir = tempfile::tempdir().expect("tempdir");
        let node_a = LayoutStore::new(dir.path(), "nodeA");
        let node_b = LayoutStore::new(dir.path(), "nodeB");

        let layout = sample_layout("nodeA");
        node_a.save(&layout).expect("save on A");

        // Node B — which never wrote it — sees node A's layout, byte-identical.
        let seen = node_b.list();
        assert_eq!(seen.len(), 1, "node B sees the layout node A saved");
        assert_eq!(seen[0], layout, "the layout tree is identical across nodes");
        assert_eq!(seen[0].origin, "nodeA", "provenance survives the sync");
        assert!(node_b.load("Dev setup").is_some(), "and is launchable on B");
    }

    #[test]
    fn the_store_unions_every_nodes_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let node_a = LayoutStore::new(dir.path(), "nodeA");
        let node_b = LayoutStore::new(dir.path(), "nodeB");
        let mut on_a = sample_layout("nodeA");
        on_a.name = "A setup".into();
        let mut on_b = sample_layout("nodeB");
        on_b.name = "B setup".into();
        node_a.save(&on_a).expect("save A");
        node_b.save(&on_b).expect("save B");

        // Either node lists BOTH (sorted by name): its own + the peer's.
        let from_a = node_a.list();
        let names: Vec<&str> = from_a.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["A setup", "B setup"]);
    }

    #[test]
    fn a_missing_store_lists_empty_rather_than_erroring() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LayoutStore::new(dir.path().join("never-synced"), "nodeA");
        assert!(store.list().is_empty());
        assert!(store.load("anything").is_none());
    }

    #[test]
    fn a_malformed_file_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LayoutStore::new(dir.path(), "nodeA");
        store
            .save(&sample_layout("nodeA"))
            .expect("save the good one");
        // Drop a corrupt file into the node dir (a half-write, a hand-edit).
        fs::write(store.node_dir().join("broken.json"), "{ not json").expect("write junk");
        let listed = store.list();
        assert_eq!(
            listed.len(),
            1,
            "the good layout still lists; junk is skipped"
        );
    }

    #[test]
    fn slugify_makes_a_safe_filename_and_never_empties() {
        assert_eq!(slugify("Dev setup"), "dev-setup");
        assert_eq!(slugify("  My/Weird…Name!!  "), "my-weird-name");
        assert_eq!(slugify("***"), "layout");
        assert_eq!(slugify(""), "layout");
    }

    #[test]
    fn overwriting_a_same_named_layout_replaces_rather_than_duplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LayoutStore::new(dir.path(), "nodeA");
        let mut layout = sample_layout("nodeA");
        store.save(&layout).expect("first save");
        layout.tabs.pop(); // change it; same name → same slug/file
        store.save(&layout).expect("overwrite");
        let listed = store.list();
        assert_eq!(listed.len(), 1, "same name overwrites, not duplicates");
        assert_eq!(listed[0].tabs.len(), 1);
    }
}
