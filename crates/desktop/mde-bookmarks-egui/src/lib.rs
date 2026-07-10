//! `mde-bookmarks-egui` — the MCNF **E12 "Quasar"** egui Bookmarks surface
//! (BOOKMARKS-4; design: `docs/design/mesh-bookmarks.md`).
//!
//! An `eframe` app on the shared [`mde_egui`] harness that reuses
//! `mde-bookmarks`' pure model + CRDT — the `Collection`/`Bookmark`/`Folder` tree
//! and the append-only `Op` set — and renders the locked three-region manager
//! (folder tree · list · detail pane) plus the enterprise addenda's left vertical
//! tab rail, all through the shared [`mde_egui::Style`] Carbon tokens (§4).
//!
//! Layering (§6): the decision logic lives in [`model`] (no egui — unit-tested
//! without a GPU); [`view`] turns that model into egui widgets. **Every** edit
//! mints a real `mde-bookmarks` op and applies it to the `Collection` — this
//! surface is glue over the model, never a re-implementation of the tree.
//!
//! Honest seams (§7): persistence + mesh sync are the BOOKMARKS-2 worker's job —
//! [`Manager::from_collection`] is the constructor it binds to; and the
//! interactive Servo browser is BOOKMARKS-5/6, so the detail pane's browser
//! region is a clearly-labelled seam, not a fake browser.

pub mod model;
pub mod view;

use std::path::PathBuf;

use mde_egui::{eframe, egui};

pub use model::Manager;
pub use view::bookmarks_panel;

/// The daemon-retained converged bookmark collection topic.
pub const STATE_COLLECTION: &str = "state/bookmarks/collection";
/// The daemon-retained status topic for bookmark dead-link checks.
pub const STATE_LINK_CHECK: &str = "state/bookmarks/link-check";
/// The daemon action topic prefix for bookmark write-side requests.
pub const ACTION_PREFIX: &str = "action/bookmarks/";
/// The daemon action topic that requests a bounded dead-link check pass.
pub const ACTION_CHECK_LINKS: &str = "action/bookmarks/check-links";

/// Edge bridge between the pure [`Manager`] and the local Bus.
pub struct BookmarksBus {
    bus_root: Option<PathBuf>,
    collection_cursor: Option<String>,
    link_check_cursor: Option<String>,
}

impl BookmarksBus {
    /// Resolve the production bus spool.
    #[must_use]
    pub fn default_root() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            collection_cursor: None,
            link_check_cursor: None,
        }
    }

    /// Construct with an explicit bus root for tests or embedding.
    #[must_use]
    pub fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self {
            bus_root,
            collection_cursor: None,
            link_check_cursor: None,
        }
    }

    /// Pump new daemon status into `manager` and publish any one-shot UI request.
    pub fn pump(&mut self, manager: &mut Manager) {
        let Some(root) = self.bus_root.clone() else {
            if manager.take_link_check_request() {
                manager.note_link_check_bus_error("no local bus spool is configured");
            }
            return;
        };
        let Ok(mut persist) = mde_bus::persist::Persist::open(root) else {
            if manager.take_link_check_request() {
                manager.note_link_check_bus_error("could not open the local bus spool");
            }
            return;
        };
        persist.reopen_if_index_changed();
        self.publish_daemon_actions(&persist, manager);
        self.read_collection_state(&persist, manager);
        self.read_link_check_status(&persist, manager);
        if manager.take_link_check_request() {
            let body = "{}";
            if let Err(e) = persist.write(
                ACTION_CHECK_LINKS,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(body),
            ) {
                manager.note_link_check_bus_error(format!("publish failed: {e}"));
            }
        }
    }

    fn publish_daemon_actions(
        &mut self,
        persist: &mde_bus::persist::Persist,
        manager: &mut Manager,
    ) {
        let mut actions = manager.take_daemon_actions();
        while !actions.is_empty() {
            let action = actions.remove(0);
            let topic = format!("{ACTION_PREFIX}{}", action.verb);
            if let Err(e) = persist.write(
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&action.body),
            ) {
                actions.insert(0, action);
                manager.prepend_daemon_actions(actions);
                manager.note_bookmark_bus_error(format!("publish failed: {e}"));
                break;
            }
        }
    }

    fn read_collection_state(
        &mut self,
        persist: &mde_bus::persist::Persist,
        manager: &mut Manager,
    ) {
        let Ok(messages) = persist.list_since(STATE_COLLECTION, self.collection_cursor.as_deref())
        else {
            return;
        };
        for msg in messages {
            self.collection_cursor = Some(msg.ulid.clone());
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            if let Ok(collection) = serde_json::from_str::<mde_bookmarks::Collection>(body) {
                manager.replace_collection(collection);
            }
        }
    }

    fn read_link_check_status(
        &mut self,
        persist: &mde_bus::persist::Persist,
        manager: &mut Manager,
    ) {
        let Ok(messages) = persist.list_since(STATE_LINK_CHECK, self.link_check_cursor.as_deref())
        else {
            return;
        };
        for msg in messages {
            self.link_check_cursor = Some(msg.ulid.clone());
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            if let Ok(status) = serde_json::from_str::<model::LinkCheckStatus>(body) {
                manager.apply_link_check_status(status);
            }
        }
    }
}

impl Default for BookmarksBus {
    fn default() -> Self {
        Self::default_root()
    }
}

/// Build the production [`Manager`] under the best-effort local identity.
///
/// The identity is the OS user and the hostname. This is the one construction
/// path for a live Bookmarks model, shared by the standalone [`BookmarksApp`] and
/// the E12 shell — the shell owns the [`Manager`] directly and mounts it with
/// [`bookmarks_panel`], so it doesn't have to know how the local author is derived.
#[must_use]
pub fn real_manager() -> Manager {
    Manager::local()
}

/// The eframe application: a single [`Manager`] rendered each frame.
pub struct BookmarksApp {
    manager: Manager,
    bus: BookmarksBus,
}

impl BookmarksApp {
    /// Build the surface over a fresh local [`Manager`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            manager: real_manager(),
            bus: BookmarksBus::default(),
        }
    }
}

impl Default for BookmarksApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for BookmarksApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.bus.pump(&mut self.manager);
        // Thin frame wrapper (E12-3, EMBED): the binary owns only the window
        // `CentralPanel`; the surface itself renders through the shared
        // [`bookmarks_panel`] fn — the exact call the E12 shell makes to mount
        // Bookmarks as an embedded panel, so standalone and embedded are identical.
        egui::CentralPanel::default().show(ctx, |ui| {
            bookmarks_panel(ui, &mut self.manager);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bookmarks::{Author, Collection, Hlc, Op, OpKind, Source};
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use uuid::Uuid;

    fn manager() -> Manager {
        Manager::new(Author::new("tester".into(), "test-node".into()))
    }

    fn daemon_collection() -> (Collection, Uuid) {
        let author = Author::new("daemon".into(), "daemon-node".into());
        let id = Uuid::from_u128(0xfeed);
        let mut collection = Collection::new();
        collection.apply(&Op::new(
            Hlc::new(100, 0, "daemon-node".into()),
            author,
            OpKind::AddBookmark {
                id,
                parent: None,
                order_key: "a".to_string(),
                url: "https://daemon.example".to_string(),
                title: "Daemon bookmark".to_string(),
                favicon_ref: None,
                tags: Vec::new(),
                notes: String::new(),
                added: 100,
                source: Source::Manual,
            },
        ));
        (collection, id)
    }

    #[test]
    fn bus_pump_publishes_check_links_request() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("bus");
        let mut manager = manager();
        let mut bus = BookmarksBus::with_root(Some(root.clone()));
        manager.request_link_check();
        bus.pump(&mut manager);

        let persist = Persist::open(root).expect("persist");
        let msgs = persist
            .list_since(ACTION_CHECK_LINKS, None)
            .expect("action messages");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].body.as_deref(), Some("{}"));
    }

    #[test]
    fn bus_pump_publishes_queued_daemon_bookmark_actions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("bus");
        let mut manager = manager();
        let mut bus = BookmarksBus::with_root(Some(root.clone()));
        let id = manager.add_bookmark("https://example.com", "Example", None);

        bus.pump(&mut manager);

        let persist = Persist::open(root).expect("persist");
        let msgs = persist
            .list_since(&format!("{ACTION_PREFIX}add"), None)
            .expect("action messages");
        assert_eq!(msgs.len(), 1);
        let body: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().expect("body")).expect("json");
        assert_eq!(body["id"].as_str().map(str::to_owned), Some(id.to_string()));
        assert_eq!(body["url"], "https://example.com");
        assert!(manager.take_daemon_actions().is_empty());
    }

    #[test]
    fn bus_pump_applies_daemon_link_check_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("bus");
        let id = uuid::Uuid::new_v4();
        let status = model::LinkCheckStatus {
            op: "bookmarks_link_check".to_string(),
            node: "daemon-node".to_string(),
            checked_at_ms: 99,
            checked: 1,
            alive: 0,
            dead: 0,
            unsupported: 0,
            errors: 1,
            truncated: false,
            records: vec![model::LinkCheckRecord {
                id,
                url: "https://err.example".to_string(),
                title: "Err".to_string(),
                health: model::LinkHealth::Error,
                http_status: None,
                detail: "curl failed".to_string(),
            }],
        };
        let body = serde_json::to_string(&status).expect("json");
        let persist = Persist::open(root.clone()).expect("persist");
        persist
            .write(STATE_LINK_CHECK, Priority::Default, None, Some(&body))
            .expect("write status");

        let mut manager = manager();
        let mut bus = BookmarksBus::with_root(Some(root));
        bus.pump(&mut manager);

        assert_eq!(manager.latest_link_check().expect("status").errors, 1);
        assert_eq!(
            manager.link_check_for(id).expect("record").detail,
            "curl failed"
        );
    }

    #[test]
    fn bus_pump_applies_daemon_collection_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("bus");
        let (collection, id) = daemon_collection();
        let body = serde_json::to_string(&collection).expect("json");
        let persist = Persist::open(root.clone()).expect("persist");
        persist
            .write(STATE_COLLECTION, Priority::Default, None, Some(&body))
            .expect("write collection");

        let mut manager = manager();
        let mut bus = BookmarksBus::with_root(Some(root));
        bus.pump(&mut manager);

        assert_eq!(manager.total(), 1);
        let Some(mde_bookmarks::Item::Bookmark(bookmark)) = manager.item(id) else {
            panic!("expected daemon bookmark");
        };
        assert_eq!(bookmark.title, "Daemon bookmark");
        assert_eq!(bookmark.url, "https://daemon.example");
    }
}
