//! `mde-editor-egui` binary — stands the Editor surface up as a client on the
//! shared harness. All the surface's logic lives in the library
//! ([`mde_editor_egui`]); this entry point only wires it into [`run_client`].

use mde_egui::{eframe, run_client};

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.Editor", "MCNF Editor", |_cc| {
        mde_editor_egui::EditorApp::new()
    })
}
