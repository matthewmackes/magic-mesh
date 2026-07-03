//! `mde-media-egui` — the MCNF media app surface (MEDIA-8,
//! `docs/design/mesh-media-player.md`).
//!
//! The full media application UI — **Sources**, **Library browse**, a **Player** view
//! (the transport + the MEDIA-6 advanced controls: speed / chapters / A-B loop /
//! frame-step / snapshot), a **Queue**, and a **`PiP` mini-player** with an auto-hide
//! OSD — on the shared [`mde_egui`] harness. It is glue over [`mde_media_core`] (§6):
//! the surface RENDERS + drives the core's [`Player`](mde_media_core::Player) /
//! [`Library`](mde_media_core::Library) / [`Playlist`](mde_media_core::Playlist) /
//! [`PlaybackControls`](mde_media_core::PlaybackControls) — it reimplements no
//! playback, indexing, or queue logic. All state lives in the core; the surface calls
//! its methods and displays its data, entirely through the shared Carbon
//! [`Style`](mde_egui::Style) tokens (§4 — no raw hex).
//!
//! # Structure
//!
//! * [`model`] — the render-agnostic [`MediaController`] (the transport glue) + the
//!   pure view folds (browse rows, source list, OSD auto-hide, time formatting). It
//!   touches no egui and drives the airgap-safe `FakeMpv` seam, so it is fully
//!   unit-tested.
//! * `app` — the egui views + the [`MediaApp`] `eframe` app. Each view is a free
//!   function headless-mount-tested (a real `Context::run` → `tessellate`, no GPU),
//!   so the surface is proven runtime-reachable in `cargo test` (§7).
//!
//! # The engine (§6/§7, honest-gated)
//!
//! The surface drives the core over a feature-selected [`Engine`]: the default build
//! is the airgap-safe [`FakeMpv`](mde_media_core::FakeMpv) (the whole transport /
//! browse / queue UI is exercised with **no system libmpv**), and `--features mpv`
//! swaps in the real mpv engine — the same honest-gated split as the core's
//! `media-smoke`. Live decode on a real GPU seat rides the DRM overlay plane (MEDIA-2)
//! and is honest-gated to a host with system libmpv, exactly like the core.
//!
//! # Jellyfin sources (MEDIA-10)
//!
//! The Sources plane wires in [`mde_jellyfin`]: a configured server browses its
//! libraries through the typed client, and selecting a title negotiates a
//! `PlaybackDecision` (direct-play / direct-stream / transcode) from the item's
//! `MediaSources` + the player's `MpvCapabilities`, then drives the core
//! [`Player`](mde_media_core::Player) through the negotiated URL and reports
//! progress. The negotiation + report construction are unit-tested; the live
//! browse / play / report legs are honest-gated to a real server.
//!
//! Tier (§6): desktop-shell — it depends only on the harness, the media core, and
//! the Jellyfin client core (all inward edges), pulling in no mesh-substrate crate.

#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

pub mod model;

mod app;

use mde_egui::{eframe, run_client};

pub use app::{media_header, media_panel, media_pump, pip_window, MediaApp};
pub use model::{
    capture_detail, client_capabilities, jellyfin_item_title, stream_media_type, CaptureUiState,
    JellyfinSession, JellyfinSourceRow, JellyfinState, MediaController, MediaTab, SourceRow,
    TransportAction, UiState,
};

/// The engine the surface drives (the real mpv engine, under `--features mpv`).
#[cfg(feature = "mpv")]
pub use mde_media_core::mpv::MpvEngine as Engine;
/// The engine the surface drives, feature-selected: the airgap-safe
/// [`FakeMpv`](mde_media_core::FakeMpv) by default, or the real mpv engine under
/// `--features mpv` (honest-gated to a host with system libmpv).
#[cfg(not(feature = "mpv"))]
pub use mde_media_core::FakeMpv as Engine;

/// Construct the default engine instance for the standalone [`MediaApp`].
#[cfg(not(feature = "mpv"))]
pub(crate) fn build_engine() -> Engine {
    mde_media_core::FakeMpv::new()
}

/// Construct the real mpv engine (honest-gated: requires system libmpv). Only built
/// under `--features mpv`, so the airgap default never links libmpv.
#[cfg(feature = "mpv")]
pub(crate) fn build_engine() -> Engine {
    mde_media_core::mpv::MpvEngine::new().expect("mpv engine init requires system libmpv")
}

/// Stand the media surface up as an `eframe` client on the shared harness. Blocks
/// until the window closes.
///
/// # Errors
/// Propagates any `eframe` startup/run failure — e.g. no display, or a wgpu
/// adapter/surface initialization failure on the host.
pub fn run() -> eframe::Result<()> {
    run_client("org.magicmesh.Media", "Media", MediaApp::new)
}
