//! `mde-media-egui` binary entry point (§7 runtime reachability): stands up the media
//! app surface. All behaviour lives in the library so the controller + view folds stay
//! unit-testable; `main` is the thin client launcher.

fn main() -> mde_egui::eframe::Result<()> {
    mde_media_egui::run()
}
