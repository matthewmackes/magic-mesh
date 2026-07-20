//! `mde-maps-location-egui` - native Maps & Location workspace.
//!
//! This crate is the first vertical slice for the vehicle-native Maps & Location
//! surface. It deliberately starts simulator-backed and local-only: no real MG90,
//! Valhalla, Nominatim, gpsd, CAN, or serial calls are faked. Instead the crate
//! exposes typed seams, guardrail models, and a polished egui workspace that can
//! launch without hardware, prove location-health behavior, and leave clear gaps
//! for the real adapters.

pub mod model;
pub mod view;

use mde_egui::{eframe, egui, run_client};

pub use model::MapsLocationSurface;
pub use view::maps_location_panel;

/// Build the production workspace state.
///
/// The first release defaults to simulator mode so the workspace is usable on a
/// clean offline seat with no MG90 attached. Real adapters will replace the
/// simulator seams without changing the shell mount point.
#[must_use]
pub fn real_maps_location() -> MapsLocationSurface {
    MapsLocationSurface::simulated()
}

/// Standalone eframe application wrapper.
pub struct MapsLocationApp {
    surface: MapsLocationSurface,
}

impl MapsLocationApp {
    /// Build the app over the same state the shell embeds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            surface: real_maps_location(),
        }
    }
}

impl Default for MapsLocationApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for MapsLocationApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            maps_location_panel(ui, &mut self.surface);
        });
    }
}

/// Run the standalone workspace as a Wayland egui client.
///
/// # Errors
/// Propagates eframe startup/runtime failures from the shared harness.
pub fn run() -> eframe::Result<()> {
    run_client("org.magicmesh.MapsLocation", "Maps & Location", |_| {
        MapsLocationApp::new()
    })
}
