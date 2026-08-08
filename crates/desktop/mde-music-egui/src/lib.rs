//! `mde-music-egui` — the MCNF **E12 "Construct"** egui music surface (E12-5).
//!
//! A daemon-projected eframe surface on the shared [`mde_egui`] harness. Catalog,
//! queue, and playback truth come from retained `mde-bus` state written by
//! `mde-musicd`; the surface emits mutations only through a host-installed,
//! authenticated Bus publisher. It never starts a provider or playback worker.
//!
//! Under E12 "Construct" the mesh-control surfaces are **panels inside the one shell**
//! (`mde-shell-egui`), not separate clients (§5, the EMBED model — there is no
//! compositor). So the complete workspace is factored into the public
//! [`music_workspace`] function: the standalone [`MusicApp`] and the shell
//! mount the same self-contained geometry and state presentation.
//!
//! Tier (§6): desktop-shell — it depends only on the harness and the music
//! service (both inward edges), pulling in no mesh-substrate crate.

pub mod model;

mod app;
#[cfg(test)]
mod menubar;
mod worker;
mod workspace_reader;

use mde_egui::{eframe, run_client};

pub use app::{music_pump, music_workspace, MusicApp};

/// Stand the music surface up as an `eframe` Wayland client on the shared
/// harness. Blocks until the window closes.
///
/// # Errors
/// Propagates any `eframe` startup/run failure — e.g. no Wayland display, or a
/// wgpu adapter/surface initialization failure on the host.
pub fn run() -> eframe::Result<()> {
    run_client("org.magicmesh.Music", "Music", MusicApp::new)
}
