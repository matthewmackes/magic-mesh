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

use mde_egui::{eframe, egui};

pub use model::Manager;
pub use view::bookmarks_panel;

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
}

impl BookmarksApp {
    /// Build the surface over a fresh local [`Manager`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            manager: real_manager(),
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
        // Thin frame wrapper (E12-3, EMBED): the binary owns only the window
        // `CentralPanel`; the surface itself renders through the shared
        // [`bookmarks_panel`] fn — the exact call the E12 shell makes to mount
        // Bookmarks as an embedded panel, so standalone and embedded are identical.
        egui::CentralPanel::default().show(ctx, |ui| {
            bookmarks_panel(ui, &mut self.manager);
        });
    }
}
