//! `mde-files-egui` — the MCNF **"Quasar"** egui file-manager surface (FILEMGR-8).
//!
//! An `eframe` app on the shared [`mde_egui`] harness that reuses `mde-files`'
//! render-agnostic core — the `Backend` trait, the `FileRow`/`Peer` model, the
//! `SendToRequest` transfer shape, and the FILEMGR-2 op queue — over the mesh Bus.
//! It is the full desktop file-manager shell: List/Grid/Details views (sortable,
//! remembered per folder, show-hidden), breadcrumbs + editable path + back/forward
//! history + tabs + dual-pane + a Places/Mesh sidebar, click/Ctrl/Shift/Ctrl-A/
//! rubber-band selection, and drag-and-drop (move default, Ctrl=copy) within and
//! between panes — every drop a real transfer through the queue with live progress.
//!
//! Layering (§6): the decision logic lives in [`model`] (no egui — unit-tested
//! without a GPU); [`ops`] wires the FILEMGR-2 queue; [`preview`] owns the
//! FILEMGR-10 thumbnail/preview decode worker + bounded caches (also egui-free);
//! [`view`] turns the model into egui widgets. The production backend is
//! `mde_files::backend::RealBackend` (local filesystem + the mesh Bus); the
//! retired Cosmic-era GUI is never pulled.

// `missing_const_for_fn` (clippy nursery) is over-eager for this crate: it flags
// trivial field getters, `&mut self` setters, and an owned-`Vec`-taking
// constructor alike as "could be const". const-ifying setters + owned-collection
// constructors is churn and a premature const commitment for no runtime win, so we
// allow this ONE nursery lint crate-wide rather than annotate ~20 sites. Every
// other clippy lint (incl. the substantive pedantic ones) stays on.
#![allow(clippy::missing_const_for_fn)]

pub mod mesh_mount;
pub mod model;
pub mod ops;
pub mod preview;
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
