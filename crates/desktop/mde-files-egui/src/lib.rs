//! `mde-files-egui` — the MCNF **E12 "Quasar"** egui file-manager surface (E12-11).
//!
//! An `eframe` app on the shared [`mde_egui`] harness that reuses `mde-files`'
//! render-agnostic core — the `Backend` trait, the `FileRow`/`Peer` model, and
//! the `SendToRequest` transfer shape — over the mesh Bus. The first slice browses
//! a local directory and a mesh-peer folder and initiates a Send-To, all rendered
//! through the shared [`mde_egui::Style`].
//!
//! Layering (§6): the decision logic lives in [`model`] (no egui — unit-tested
//! without a GPU); [`view`] turns that model into egui widgets. The production
//! backend is `mde_files::backend::RealBackend` (local filesystem + the mesh Bus);
//! the retired Cosmic-era file-manager GUI is never pulled (`mde-files` is
//! consumed with its `gui` feature off).

pub mod model;
pub mod view;

use mde_egui::{eframe, egui};
use mde_files::backend::RealBackend;

pub use model::FileBrowser;
pub use view::files_panel;

/// Build the production [`FileBrowser`] over the [`RealBackend`] — the local
/// filesystem for local panes and the mesh Bus for peer panes + Send-To.
///
/// This is the one construction path for a live Files model, shared by the
/// standalone [`FilesApp`] and the E12 shell (`mde-shell-egui`, E12-3b), which
/// owns the [`FileBrowser`] directly and mounts it with [`files_panel`]. Factored
/// out because [`FileBrowser::new`] takes a `Box<dyn Backend>` and only this crate
/// knows the production backend — so the shell doesn't have to depend on
/// `mde-files` to build one.
#[must_use]
pub fn real_browser() -> FileBrowser {
    FileBrowser::new(Box::new(RealBackend::new()))
}

/// The eframe application: a single [`FileBrowser`] rendered each frame.
pub struct FilesApp {
    browser: FileBrowser,
}

impl FilesApp {
    /// Build the surface over the production [`RealBackend`] — the local
    /// filesystem for local panes and the mesh Bus for peer panes + Send-To.
    #[must_use]
    pub fn new() -> Self {
        Self {
            browser: real_browser(),
        }
    }
}

impl Default for FilesApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for FilesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Thin frame wrapper (E12-3, EMBED): the binary only owns the window
        // `CentralPanel`; the surface itself renders through the shared
        // [`files_panel`] fn — the exact same call the E12 shell makes to mount
        // Files as an embedded panel, so standalone and embedded are identical.
        egui::CentralPanel::default().show(ctx, |ui| {
            files_panel(ui, &mut self.browser);
        });
    }
}
