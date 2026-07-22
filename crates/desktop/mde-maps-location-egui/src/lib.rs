//! `mde-maps-location-egui` - native Maps & Location workspace.
//!
//! Production is LIVE-ONLY (WL-UX-007/S1, operator directive 2026-07-22;
//! PLATFORM-INTERFACES P8/Q33: absent data reads absent, never fabricated). The
//! workspace boots on [`MapsLocationSurface::live`] — honest-empty everywhere —
//! and populates exclusively from:
//!
//! * the MG90 vehicle-gateway mirror (`state/vehicle/<node>` on the Bus, folded
//!   by [`MapsLocationSurface::refresh_from_bus`]), and
//! * real on-disk artifacts (the deployed `MBTiles` basemap + gazetteer under
//!   the maps data dir — see [`basemap`] and [`geocode`]).
//!
//! The former simulator seed survives only as a cfg-gated test fixture
//! (`MapsLocationSurface::simulated`, `#[cfg(any(test, feature = "sim-fixture"))]`);
//! no production build compiles it, so no production path can show dummy data.

pub mod airspace;
pub mod basemap;
pub mod car_status;
pub mod geocode;
pub mod model;
pub mod view;

use mde_egui::{eframe, egui, run_client};

pub use car_status::{CarStatusItem, CarStatusSelection};
pub use model::MapsLocationSurface;
pub use view::maps_location_panel;

/// Build the production workspace state.
///
/// // PLATFORM-INTERFACES P8/Q33 — operator directive 2026-07-22 (WL-UX-007/S1):
/// production boots on [`MapsLocationSurface::live`] — honest-empty everywhere,
/// never the simulator seed — then folds a live `state/vehicle/<node>` mirror
/// on top when one is retained on the Bus for this host.
/// [`MapsLocationSurface::refresh_from_bus`] is fail-soft, so a seat with no
/// adapter worker (or no Bus spool at all) keeps the honest empty state: an
/// acquiring GNSS primary, zero airspace contacts, absent telemetry — not an
/// error, and not fake data. The shell re-folds every frame; this seeds the
/// standalone app.
#[must_use]
pub fn real_maps_location() -> MapsLocationSurface {
    let mut surface = MapsLocationSurface::live();
    surface.refresh_from_bus(&local_node_id());
    surface
}

/// This host's node id, for the `state/vehicle/<node>` mirror topic.
///
/// The standalone app (unlike the shell, which already tracks its own
/// `local_host`) has no caller-supplied node id, so it resolves the same way
/// the shell's `local_hostname()` does: `$HOSTNAME`, falling back to
/// `/etc/hostname`, falling back to the literal `"local"` (never panics).
fn local_node_id() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "local".to_string())
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
