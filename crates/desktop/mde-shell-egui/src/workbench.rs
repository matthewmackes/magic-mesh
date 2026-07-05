//! The Workbench — the five-plane mesh-control nav the chrome bar expands into.
//!
//! E12-3 shipped the *skeleton*: the five scope-first planes as a selectable rail
//! beside an honest content pane. Live data then wires into each plane: **This
//! Node** (WB-ThisNode — this host's role / overlay IP / presence + heartbeat /
//! daemon health, off the mesh-status snapshot), **Cloud** (QC-12 — the
//! QUASAR-CLOUD Q70 lock: *the Controller plane BECOMES the Cloud plane*; the
//! five cloud resource kinds + the full launch picker + fleet-state launch
//! presets over the typed QC-11 verbs, with the old Controller view — the
//! elected leader + control-service rollup — kept live inside it, collapsed),
//! **Network** (WB-Network — the overlay IP + cipher, elected leader, the peer
//! directory as network links, and overlay routing), **Fleet** (MV-6 — per-node
//! KVM reality off the Bus), and **Provisioning** (WB-Provisioning — per-node
//! deployment tier + role rollup, the fleet version target vs each node's build,
//! and per-node enrollment readiness) — all five planes are live off the
//! mesh-status snapshot / Bus. Nothing here fakes a metric (governance §7) — a
//! plane shows live data or an honest blurb, never stand-in data.

use mde_egui::egui::{self, RichText};
use mde_egui::Style;

// QC-12 — the Cloud plane's own module file, mounted here (rather than from
// `main.rs`) because the crate root is owned by parallel wave units; the
// `#[path]` mount keeps the plane in its own file while this module stays the
// one place the planes meet.
#[path = "cloud_plane.rs"]
pub mod cloud_plane;

/// One of the five top-level control planes of the Workbench, ordered by blast
/// radius — from the local host outward to the whole fleet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Plane {
    /// This host — hardware, the local desktop seat, node-local services.
    #[default]
    ThisNode,
    /// The mesh cloud — `OpenStack` self-service for every member (QC-12; the
    /// Q70 lock renamed the Controller plane: `OpenStack` IS the control brain
    /// the old plane described).
    Cloud,
    /// Network fabric — the Nebula overlay, lighthouses, routes, reachability.
    Network,
    /// The fleet — every peer and the VM desktops they serve.
    Fleet,
    /// Provisioning — golden images, enrollment, bringing new nodes online.
    Provisioning,
}

impl Plane {
    /// The five planes in nav order (local host → fleet-wide).
    pub(crate) const ALL: [Self; 5] = [
        Self::ThisNode,
        Self::Cloud,
        Self::Network,
        Self::Fleet,
        Self::Provisioning,
    ];

    /// The short rail label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ThisNode => "This Node",
            Self::Cloud => "Cloud",
            Self::Network => "Network",
            Self::Fleet => "Fleet",
            Self::Provisioning => "Provisioning",
        }
    }

    /// A one-line, honest description of what the plane will host. Descriptive
    /// copy only — never a stand-in for live data (§7).
    pub(crate) const fn blurb(self) -> &'static str {
        match self {
            Self::ThisNode => {
                "This host — hardware, the local desktop seat, and node-local services."
            }
            Self::Cloud => {
                "The mesh cloud — instances, volumes, images, networks, and stacks, \
                 self-served by every member."
            }
            Self::Network => {
                "Mesh fabric — the Nebula overlay, lighthouses, routes, and reachability."
            }
            Self::Fleet => "Every peer in the mesh and the VM desktops they serve.",
            Self::Provisioning => "Golden images, node enrollment, and bringing new peers online.",
        }
    }
}

/// Render the expanded Workbench: a title, the plane rail, and the selected
/// plane's content pane. `selected` is read and written, so a rail click changes
/// the active plane. The This Node plane renders this host's live status from
/// `thisnode` (WB-ThisNode), the Cloud plane (QC-12) renders the mesh cloud off
/// the typed QC-11 Bus verbs (its mutable picker/arming state rides the egui
/// data store — `cloud_plane::state_handle` — because this signature is frozen
/// this wave; the old Controller view stays live inside it via `controller`),
/// the Network plane from `network` (WB-Network), the Fleet plane's live
/// per-node KVM reality from `datacenter` (MV-6), and the Provisioning plane's
/// live deployment / version / enrollment posture from `provisioning`
/// (WB-Provisioning) — every plane renders live status. The Provisioning plane
/// additionally hosts two Bus-driven onboarding flows: the Spawn Lighthouse flow
/// (`spawn_lighthouse`, OW-7) — promote a LAN-only mesh by standing up its first
/// lighthouse + migrating the CA — and the day-2 Services flow (`services`,
/// OW-11): pick Music/Files/Voice, preview the daemon's plan, apply over the Bus.
// One state struct per mounted plane view — the Workbench is the single place
// they all meet, so the arity IS the plane count, not a design smell.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    selected: &mut Plane,
    datacenter: &mut crate::datacenter::DatacenterState,
    // Read-only: the This Node / Network / Provisioning planes only render
    // their polled status (`&self`), unlike the Fleet plane whose `datacenter`
    // publishes lifecycle actions. `controller` is read-only too — the Cloud
    // plane embeds its view and keeps its own mutable state in egui memory.
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
    // Mutable like `services`: the Spawn Lighthouse flow (OW-7) publishes
    // spawn-lighthouse requests onto the Bus and holds the daemon's typed answer.
    spawn_lighthouse: &mut crate::spawn_lighthouse_flow::SpawnLighthouseFlowState,
) {
    // MENUBAR-ALL — the shared top bar (WORKBENCH). Its one honest menu is the
    // **Plane** switch — the same `selected` seam the rail below drives (§6, no new
    // behaviour); a pick radio-jumps the active plane. The bar's UPPERCASE display
    // title replaces the old proportional heading (design lock 2).
    if let Some(plane) = menubar::show(ui, *selected) {
        *selected = plane;
    }
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
                // QC-12 — the Cloud plane (the Q70 lock: the Controller plane
                // BECOMES the Cloud plane). Renders the mesh cloud off the typed
                // QC-11 Bus verbs; its mutable picker/arming/poll state rides the
                // egui data store because this signature is frozen this wave (see
                // the cloud_plane module docs). The old Controller view stays
                // live inside the plane, collapsed (§6 reuse — the leader lease
                // the leader-hosted MariaDB follows is part of the cloud story).
                Plane::Cloud => {
                    let handle = cloud_plane::state_handle(ui);
                    let mut cloud = handle
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    cloud_plane::show(ui, &mut cloud, controller);
                }
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
                    // OW-7 — promote a LAN-only mesh: spawn its first lighthouse +
                    // migrate the CA (the durable off-desktop CA home is provisioning
                    // work), over the Bus against the spawn_lighthouse_onboard worker.
                    spawn_lighthouse.show(ui);
                    ui.add_space(Style::SP_M);
                    ui.separator();
                    ui.add_space(Style::SP_M);
                    services.show(ui);
                }
            }
        });
    });
}

/// MENUBAR-ALL (Workbench) — the shared top bar over the five-plane nav.
///
/// The Workbench is a pure *navigation* surface: its content is whichever of the
/// five [`Plane`]s the operator selects, and its every real control is that plane
/// switch (the sub-flows the Provisioning plane hosts live inside the plane body,
/// not the bar). So the bar carries exactly one honest menu — **Plane** — whose
/// items are the mouse twin of the rail's `selectable_label`s (§6, one seam), each
/// radio-checked to the live selection. There is no File/Edit/Help spine to invent
/// (the surface has no file, clipboard, or about seam), so it is honestly omitted
/// (§7 — no dead entries). The status cluster names the active plane (live state).
mod menubar {
    use super::Plane;
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// Render the WORKBENCH bar and return the plane the operator picked this frame,
    /// if any — the same seam the plane rail drives (`*selected = plane`).
    pub(super) fn show(ui: &mut Ui, active: Plane) -> Option<Plane> {
        let menus = build_menus(active);
        let status = build_status(active);
        let model = MenuBarModel {
            // The dock tints the Workbench lead cell with the brand accent (its
            // standalone-lead cell has no group hue), so the title matches (lock 2).
            title: "Workbench",
            accent: Style::ACCENT,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// The **Plane** menu: one radio item per [`Plane`] in blast-radius order, the
    /// active one checked — every item drives the real `selected` seam.
    fn build_menus(active: Plane) -> Vec<Menu<Plane>> {
        let items: Vec<Entry<Plane>> = Plane::ALL
            .iter()
            .map(|&p| Entry::Item(Item::new(p, p.label()).checked(active == p)))
            .collect();
        vec![Menu::new("Plane", items)]
    }

    /// The live status cluster: the active plane's name (which plane is showing).
    fn build_status(active: Plane) -> Vec<StatusChip> {
        vec![StatusChip::new(active.label(), ChipTone::Info)]
    }

    #[cfg(test)]
    mod tests {
        use super::{build_menus, build_status, Plane};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

        #[test]
        fn plane_menu_lists_every_plane_with_the_active_one_checked() {
            let menus = build_menus(Plane::Network);
            assert_eq!(menus.len(), 1, "the Workbench carries one honest menu");
            let plane_menu = &menus[0];
            assert_eq!(plane_menu.title, "Plane");
            let ids: Vec<Plane> = plane_menu
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i.id),
                    _ => None,
                })
                .collect();
            assert_eq!(
                ids,
                Plane::ALL.to_vec(),
                "every plane is reachable, in order"
            );
            // Exactly the active plane is checked (radio) — the rest are unchecked,
            // never omitted (§7).
            for entry in &plane_menu.entries {
                if let Entry::Item(item) = entry {
                    assert_eq!(
                        item.checked,
                        Some(item.id == Plane::Network),
                        "{:?} check-state must track the active plane",
                        item.id
                    );
                }
            }
        }

        #[test]
        fn status_names_the_active_plane() {
            let chips = build_status(Plane::Provisioning);
            assert!(chips
                .iter()
                .any(|c| c.text == "Provisioning" && c.tone == ChipTone::Info));
        }

        #[test]
        fn menu_bar_renders_headless() {
            use mde_egui::egui::{self, pos2, vec2, Rect};
            use mde_egui::Style;
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = super::show(ui, Plane::ThisNode);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(
                !prims.is_empty(),
                "the Workbench bar produced no primitives"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Plane;

    #[test]
    fn there_are_five_planes_in_blast_radius_order() {
        assert_eq!(Plane::ALL.len(), 5);
        assert_eq!(Plane::ALL[0], Plane::ThisNode);
        // QC-12 / Q70 — the Controller slot is now the Cloud plane.
        assert_eq!(Plane::ALL[1], Plane::Cloud);
        assert_eq!(Plane::ALL[4], Plane::Provisioning);
    }

    #[test]
    fn the_controller_plane_became_the_cloud_plane() {
        // The Q70 lock: no plane is labelled Controller any more, and the Cloud
        // plane is reachable with an honest cloud blurb.
        assert!(Plane::ALL.iter().all(|p| p.label() != "Controller"));
        assert_eq!(Plane::Cloud.label(), "Cloud");
        assert!(Plane::Cloud.blurb().contains("instances"));
        assert!(Plane::Cloud.blurb().contains("stacks"));
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
