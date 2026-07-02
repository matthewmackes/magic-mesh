//! `mde-bookmarks-egui` binary — stands the Bookmarks surface up as a client on
//! the shared harness. All the surface's logic lives in the library
//! ([`mde_bookmarks_egui`]); this entry point only wires it into [`run_client`].

use mde_egui::{eframe, run_client};

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.Bookmarks", "MCNF Bookmarks", |_cc| {
        mde_bookmarks_egui::BookmarksApp::new()
    })
}
