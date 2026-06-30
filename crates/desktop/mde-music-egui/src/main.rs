//! `mde-music-egui` binary entry point (§0.12 runtime reachability): stands up the
//! egui music surface. All behaviour lives in the library so the view-model stays
//! unit-testable; `main` is the thin Wayland-client launcher.

fn main() -> mde_egui::eframe::Result<()> {
    mde_music_egui::run()
}
