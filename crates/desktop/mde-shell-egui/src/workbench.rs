//! The Workbench — the five-plane mesh-control nav the chrome bar expands into.
//!
//! E12-3 shipped the *skeleton*: the five scope-first planes as a selectable rail
//! beside an honest content pane. Live data then wires into each plane: **This
//! Node** (WB-ThisNode — this host's role / overlay IP / presence + heartbeat /
//! daemon health, off the mesh-status snapshot), **Controller** (WB-Controller —
//! the elected controller + its leader lease and the fleet-wide control-service
//! health rollup), **Network** (WB-Network — the overlay IP + cipher, elected
//! leader, the peer directory as network links, and overlay routing), **Fleet**
//! (MV-6 — per-node KVM reality off the Bus), and **Provisioning** (WB-Provisioning
//! — per-node deployment tier + role rollup, the fleet version target vs each
//! node's build, and per-node enrollment readiness) — all five planes are now
//! live off the mesh-status snapshot / Bus. Nothing here fakes a metric (governance
//! §7) — a plane shows live data or an honest blurb, never stand-in data.

use mde_egui::egui::{self, RichText};
use mde_egui::Style;

/// One of the five top-level control planes of the Workbench, ordered by blast
/// radius — from the local host outward to the whole fleet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Plane {
    /// This host — hardware, the local desktop seat, node-local services.
    #[default]
    ThisNode,
    /// The mesh controller — leader election, etcd state, controller health.
    Controller,
    /// Network fabric — the Nebula overlay, lighthouses, routes, reachability.
    Network,
    /// The fleet — every peer and the VM desktops they serve.
    Fleet,
    /// Provisioning — golden images, enrollment, bringing new nodes online.
    Provisioning,
}

impl Plane {
    /// The five planes in nav order (local host → fleet-wide).
    pub(crate) const ALL: [Plane; 5] = [
        Plane::ThisNode,
        Plane::Controller,
        Plane::Network,
        Plane::Fleet,
        Plane::Provisioning,
    ];

    /// The short rail label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Plane::ThisNode => "This Node",
            Plane::Controller => "Controller",
            Plane::Network => "Network",
            Plane::Fleet => "Fleet",
            Plane::Provisioning => "Provisioning",
        }
    }

    /// A one-line, honest description of what the plane will host. Descriptive
    /// copy only — never a stand-in for live data (§7).
    pub(crate) const fn blurb(self) -> &'static str {
        match self {
            Plane::ThisNode => {
                "This host — hardware, the local desktop seat, and node-local services."
            }
            Plane::Controller => {
                "Control plane — leader election, etcd state, and controller health."
            }
            Plane::Network => {
                "Mesh fabric — the Nebula overlay, lighthouses, routes, and reachability."
            }
            Plane::Fleet => "Every peer in the mesh and the VM desktops they serve.",
            Plane::Provisioning => "Golden images, node enrollment, and bringing new peers online.",
        }
    }
}

/// Render the expanded Workbench: a title, the plane rail, and the selected
/// plane's content pane. `selected` is read and written, so a rail click changes
/// the active plane. The This Node plane renders this host's live status from
/// `thisnode` (WB-ThisNode), the Controller plane from `controller` (WB-Controller),
/// the Network plane from `network` (WB-Network), the Fleet plane's live per-node
/// KVM reality from `datacenter` (MV-6), and the Provisioning plane's live
/// deployment / version / enrollment posture from `provisioning` (WB-Provisioning)
/// — every plane now renders live status. The Provisioning plane additionally
/// hosts the day-2 Services flow (`services`, OW-11): pick Music/Files/Voice,
/// preview the daemon's plan, apply over the Bus.
// One state struct per mounted plane view — the Workbench is the single place
// they all meet, so the arity IS the plane count, not a design smell.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    selected: &mut Plane,
    datacenter: &mut crate::datacenter::DatacenterState,
    // Read-only: the This Node / Controller / Network / Provisioning planes only
    // render their polled status (`&self`), unlike the Fleet plane whose
    // `datacenter` publishes lifecycle actions.
    thisnode: &crate::thisnode::ThisNodeState,
    // Mutable: the SURFACE-6 card reads the surface workers' typed state off the
    // Bus and publishes typed enable / fw-apply requests (it holds the in-flight
    // arm inputs + the in-process display controller).
    surface_card: &mut crate::surface_card::SurfaceCardState,
    network: &crate::network::NetworkState,
    controller: &crate::controller::ControllerState,
    provisioning: &crate::provisioning::ProvisioningState,
    // Mutable like `datacenter`: the Services flow publishes service-add
    // requests onto the Bus and holds the daemon's typed answer.
    services: &mut crate::services_flow::ServicesFlowState,
) {
    ui.add_space(Style::SP_L);
    ui.heading(
        RichText::new("Workbench")
            .color(Style::TEXT)
            .size(Style::HEADING),
    );
    ui.colored_label(
        Style::TEXT_DIM,
        "Mesh control — expanded from the chrome bar.",
    );
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_M);

    ui.horizontal_top(|ui| {
        // ── Plane rail (selectable) ──────────────────────────────────────────
        ui.vertical(|ui| {
            ui.set_min_width(Style::SP_XL * 6.0);
            for plane in Plane::ALL {
                if ui
                    .selectable_label(*selected == plane, plane.label())
                    .clicked()
                {
                    *selected = plane;
                }
                ui.add_space(Style::SP_XS);
            }
        });

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_M);

        // ── Content pane for the selected plane ──────────────────────────────
        ui.vertical(|ui| {
            ui.label(
                RichText::new(selected.label())
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::TEXT_DIM, selected.blurb());
            ui.add_space(Style::SP_M);
            // Every plane is matched explicitly — no `_` wildcard — so a future
            // plane variant can't silently fall through to a placeholder (clippy's
            // `match_wildcard_for_single_variants` fix once only one arm remained).
            match *selected {
                // WB-ThisNode — this host's live status (role, overlay IP,
                // presence + heartbeat, daemon health, peer/leader context) off the
                // world-readable mesh-status snapshot.
                Plane::ThisNode => {
                    thisnode.show(ui);
                    // SURFACE-6 — the model-gated Surface / Hardware Enablement
                    // card. It draws only on a detected Surface (the summary
                    // topic is the gate); on every other node it's inert.
                    if surface_card.is_surface() {
                        surface_card.show(ui);
                    }
                }
                // WB-Controller — the mesh control plane's live status (the elected
                // controller + its leader lease, and the fleet-wide control-service
                // health rollup) off the same snapshot.
                Plane::Controller => controller.show(ui),
                // WB-Network — the mesh fabric's live status (overlay IP + cipher,
                // elected leader, the peer directory as network links, network
                // service health, overlay routing) off the same snapshot.
                Plane::Network => network.show(ui),
                // MV-6 — the Fleet plane drives live KVM host health + the VM
                // roster off the Bus (Podman container rows follow once MV-4 lands).
                Plane::Fleet => datacenter.show(ui),
                // WB-Provisioning — the mesh's live deployment posture (per-node
                // tier + role rollup, the fleet version target vs each node's build
                // + update flag, per-node enrollment readiness) off the same
                // snapshot — plus the OW-11 Services flow (day-2 service adds are
                // provisioning work: `onboard service-add` over the Bus).
                Plane::Provisioning => {
                    provisioning.show(ui);
                    ui.add_space(Style::SP_M);
                    ui.separator();
                    ui.add_space(Style::SP_M);
                    services.show(ui);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::Plane;

    #[test]
    fn there_are_five_planes_in_blast_radius_order() {
        assert_eq!(Plane::ALL.len(), 5);
        assert_eq!(Plane::ALL[0], Plane::ThisNode);
        assert_eq!(Plane::ALL[4], Plane::Provisioning);
    }

    #[test]
    fn plane_labels_and_blurbs_are_present_and_distinct() {
        for p in Plane::ALL {
            assert!(!p.label().is_empty(), "{p:?} has an empty label");
            // A blurb is real descriptive copy, longer than its one-word label —
            // not a stub (§7).
            assert!(p.blurb().len() > p.label().len(), "{p:?} blurb too short");
        }
        let mut labels: Vec<&str> = Plane::ALL.iter().map(|p| p.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 5, "plane labels must be distinct");
    }

    #[test]
    fn this_node_is_the_default_plane() {
        assert_eq!(Plane::default(), Plane::ThisNode);
    }
}
