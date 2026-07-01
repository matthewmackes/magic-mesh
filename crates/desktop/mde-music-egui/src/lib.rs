//! `mde-music-egui` — the MCNF **E12 "Quasar"** egui music surface (E12-5).
//!
//! A standalone eframe surface on the shared [`mde_egui`] harness that REUSES the
//! `mde-musicd` service crate end-to-end (governance §6 — glue, not
//! reimplementation):
//!
//! * the Subsonic/Airsonic REST [`mde_musicd::airsonic::Client`] lists the album
//!   library and builds the authenticated `stream` URL,
//! * the shared [`mde_musicd::creds`] loader supplies the server + credentials,
//! * the codec classifier + native [`mde_musicd::engine::Engine`] play the track.
//!
//! Everything renders through the shared [`mde_egui::Style`]. The async airsonic
//! calls and the audio engine live on a [`worker`] thread so the egui UI thread
//! never blocks; the render-agnostic view-model in [`model`] is unit-tested
//! without a GPU or a sound device.
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels inside the one shell**
//! (`mde-shell-egui`), not separate clients (§5, the EMBED model — there is no
//! compositor). So the central view is factored into the public [`music_panel`]
//! function: the standalone [`MusicApp`] renders it into its own `CentralPanel`,
//! and the shell renders the *same* function into a panel of its egui context, so
//! the surface looks and behaves identically either way.
//!
//! Tier (§6): desktop-shell — it depends only on the harness and the music
//! service (both inward edges), pulling in no mesh-substrate crate.

pub mod model;

mod app;
mod worker;

use mde_egui::{eframe, run_client};

pub use app::{music_header, music_panel, music_pump, MusicApp};

/// Stand the music surface up as an `eframe` Wayland client on the shared
/// harness. Blocks until the window closes.
///
/// # Errors
/// Propagates any `eframe` startup/run failure — e.g. no Wayland display, or a
/// wgpu adapter/surface initialization failure on the host.
pub fn run() -> eframe::Result<()> {
    run_client("org.magicmesh.Music", "Music", MusicApp::new)
}
