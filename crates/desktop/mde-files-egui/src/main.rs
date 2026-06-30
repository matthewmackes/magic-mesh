//! `mde-files-egui` binary — stands the Files surface up as a Wayland client on
//! the shared harness. All the surface's logic lives in the library
//! ([`mde_files_egui`]); this entry point only wires it into [`run_client`].

use mde_egui::{eframe, run_client};

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.Files", "MCNF Files", |_cc| {
        mde_files_egui::FilesApp::new()
    })
}
