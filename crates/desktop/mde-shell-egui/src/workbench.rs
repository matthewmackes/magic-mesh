//! The Workbench — the five-plane mesh-control nav the chrome bar expands into.
//!
//! E12-3 ships the *skeleton*: the five scope-first planes as a selectable rail
//! beside an honest, descriptive content pane. Live Bus data (peers, sessions,
//! provisioning state) wires into each plane in a later unit; nothing here fakes
//! a metric (governance §7) — the blurbs are descriptive copy, not stand-in data.

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
/// the active plane. The Fleet plane renders live per-node KVM reality from
/// `datacenter` (MV-6); the other planes still show descriptive copy.
pub(crate) fn show(
    ui: &mut egui::Ui,
    selected: &mut Plane,
    datacenter: &mut crate::datacenter::DatacenterState,
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
            if *selected == Plane::Fleet {
                // MV-6 — the Fleet plane drives live KVM host health + the VM
                // roster off the Bus (Podman container rows follow once MV-4 lands).
                datacenter.show(ui);
            } else {
                ui.colored_label(
                    Style::TEXT_DIM,
                    "Live mesh data wires into this plane in a later unit.",
                );
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
