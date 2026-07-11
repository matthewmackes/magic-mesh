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
/// the typed QC-11 Bus verbs (its mutable picker/arming state is the Shell-owned
/// `cloud` field, borrowed like every other surface; the old Controller view
/// stays live inside it via `controller`),
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
    // Mutable: the Cloud plane (QC-12) holds its own poll/picker/arming state,
    // borrowed from the Shell field like every other surface (it embeds the
    // read-only `controller` view collapsed inside it).
    cloud: &mut cloud_plane::CloudPlaneState,
    provisioning: &crate::provisioning::ProvisioningState,
    // Mutable like `datacenter`: the Services flow publishes service-add
    // requests onto the Bus and holds the daemon's typed answer.
    services: &mut crate::services_flow::ServicesFlowState,
    // Mutable like `services`: the Spawn Lighthouse flow (OW-7) publishes
    // spawn-lighthouse requests onto the Bus and holds the daemon's typed answer.
    spawn_lighthouse: &mut crate::spawn_lighthouse_flow::SpawnLighthouseFlowState,
) {
    // MENU-1 — the shared top bar, retitled **State of the Mesh** (operator
    // retitle; the `Surface` enum name stays Workbench). The full MenuBarModel:
    // **View** (plane navigation — the same `selected` seam the rail below
    // drives), the **active plane's verb menu** (Cloud verbs into the
    // egui-memory Cloud state; Fleet → Refresh onto the datacenter poll seam),
    // and **Help** (the bar-owned plane guide). The status cluster carries live
    // mesh state — active plane · elected leader · peer count · fleet update
    // target — each chip only when the fact is live (§7).
    if let Some(action) = menubar::show(ui, *selected, controller, provisioning) {
        match action {
            menubar::MenuAction::Plane(plane) => *selected = plane,
            menubar::MenuAction::Cloud(verb) => {
                // The same Shell-owned state the Cloud plane body drives — a
                // menu pick opens the exact affordance the body's button opens
                // (§6, one seam), and View lands on the plane it acted on.
                cloud.apply_menu_verb(verb);
                *selected = Plane::Cloud;
            }
            menubar::MenuAction::FleetRefresh => datacenter.refresh_now(),
            // Handled inside the bar (it owns the guide window's open flag);
            // unreachable here, kept explicit so a new action can't fall
            // through silently.
            menubar::MenuAction::HelpGuide => {}
        }
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
                // QC-11 Bus verbs; its mutable picker/arming/poll state is the
                // Shell-owned `cloud` field, borrowed here like every other
                // surface. The old Controller view stays live inside the plane,
                // collapsed (§6 reuse — the leader lease the leader-hosted
                // MariaDB follows is part of the cloud story).
                Plane::Cloud => cloud_plane::show(ui, cloud, controller),
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

/// MENU-1 — the **State of the Mesh** bar over the five-plane nav (the operator
/// retitle of the old one-menu WORKBENCH bar, which is retired).
///
/// The full shared-`MenuBar` treatment, at plane depth (the `IaC` IAC-5 bar is
/// the reference idiom):
///
/// - **View** — plane navigation: one radio item per [`Plane`] in blast-radius
///   order, the mouse twin of the rail's `selectable_label`s (§6, one seam).
/// - **The active plane's verb menu** — only for a plane with a real mutable
///   seam behind this bar: **Cloud** (the QC-12 verbs, applied to the plane's
///   egui-memory state — refresh + the launch picker + the per-kind New forms)
///   and **Fleet** (Refresh now, onto the datacenter poll seam). The This Node /
///   Network / Provisioning planes render read-only snapshot views behind
///   `&self` here — no mutable verb seam exists, so no verb menu is invented
///   (§7 — honest omission, never a dead entry; the Provisioning flows carry
///   their own in-body controls).
/// - **Help** — the bar-owned **Plane Guide** window (the five planes + their
///   real blurbs; the voice bar's bar-owned-overlay idiom).
///
/// The status cluster is live mesh state: the active plane, the elected
/// leader + peer count (the controller's parsed snapshot — chips gated on
/// `ControllerState::seen`, §7), and the fleet update target (provisioning).
mod menubar {
    use super::cloud_plane::CloudMenuVerb;
    use super::Plane;
    use crate::controller::ControllerState;
    use crate::provisioning::ProvisioningState;
    use mde_egui::egui::{self, Ui};
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// The shared filled-circle chip icon (the datacenter / Instances glyph).
    const DOT: &str = "\u{25CF}";

    /// One bar action. `HelpGuide` is intercepted by [`show`] (the bar owns the
    /// guide window); the rest dispatch in `workbench::show`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// View → jump to this plane (the rail's `selected` seam).
        Plane(Plane),
        /// Cloud → a QC-12 verb into the Cloud plane's egui-memory state.
        Cloud(CloudMenuVerb),
        /// Fleet → queue an immediate datacenter re-read.
        FleetRefresh,
        /// Help → toggle the bar-owned Plane Guide window.
        HelpGuide,
    }

    /// Render the STATE OF THE MESH bar and return the action picked this
    /// frame, if any (Help is handled here — the bar owns the guide window).
    pub(super) fn show(
        ui: &mut Ui,
        active: Plane,
        controller: &ControllerState,
        provisioning: &ProvisioningState,
    ) -> Option<MenuAction> {
        let menus = build_menus(active);
        let status = build_status(active, controller, provisioning);
        let model = MenuBarModel {
            // The dock tints the Workbench lead cell with the brand accent (its
            // standalone-lead cell has no group hue), so the title matches.
            title: "State of the Mesh",
            accent: Style::ACCENT,
            menus: &menus,
            status: &status,
        };
        let picked = MenuBar::show(ui, &model);

        // The Plane Guide is bar-owned UI: a Help pick flips the persisted
        // flag; the window renders while it is set (the egui-memory idiom the
        // Explorer lens uses).
        let guide_id = egui::Id::new("workbench-plane-guide");
        let mut open = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(guide_id).unwrap_or(false));
        let picked = match picked {
            Some(MenuAction::HelpGuide) => {
                open = !open;
                None
            }
            other => other,
        };
        if open {
            render_plane_guide(ui, &mut open);
        }
        ui.ctx().data_mut(|d| d.insert_temp(guide_id, open));
        picked
    }

    /// The Help → Plane Guide window: every plane's label + its real blurb —
    /// the same honest copy the content pane renders (§6, one source).
    fn render_plane_guide(ui: &Ui, open: &mut bool) {
        egui::Window::new("Plane guide")
            .open(open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                mde_egui::muted_note(
                    ui,
                    "The Workbench's five control planes, ordered by blast radius.",
                );
                ui.add_space(Style::SP_S);
                for plane in Plane::ALL {
                    ui.label(
                        egui::RichText::new(plane.label())
                            .color(Style::TEXT)
                            .size(Style::BODY)
                            .strong(),
                    );
                    mde_egui::muted_note(ui, plane.blurb());
                    ui.add_space(Style::SP_XS);
                }
            });
    }

    /// Build the bar's menus for the active plane: View (plane radios), the
    /// active plane's verb menu when one exists, and Help.
    fn build_menus(active: Plane) -> Vec<Menu<MenuAction>> {
        let view: Vec<Entry<MenuAction>> = Plane::ALL
            .iter()
            .map(|&p| Entry::Item(Item::new(MenuAction::Plane(p), p.label()).checked(active == p)))
            .collect();
        let mut menus = vec![Menu::new("View", view)];

        match active {
            // The Cloud plane's verbs (QC-12) — each the mouse twin of an
            // affordance the plane body renders (§6, one seam).
            Plane::Cloud => menus.push(Menu::new(
                "Cloud",
                vec![
                    Entry::Item(Item::new(
                        MenuAction::Cloud(CloudMenuVerb::Refresh),
                        "Refresh now",
                    )),
                    Entry::Item(Item::new(
                        MenuAction::Cloud(CloudMenuVerb::LaunchInstance),
                        "Launch instance\u{2026}",
                    )),
                    Entry::Item(Item::new(
                        MenuAction::Cloud(CloudMenuVerb::NewVolume),
                        "New volume\u{2026}",
                    )),
                    Entry::Item(Item::new(
                        MenuAction::Cloud(CloudMenuVerb::RegisterImage),
                        "Register image\u{2026}",
                    )),
                    Entry::Item(Item::new(
                        MenuAction::Cloud(CloudMenuVerb::NewNetwork),
                        "New network\u{2026}",
                    )),
                ],
            )),
            // The Fleet plane's one honest verb: queue an immediate re-read of
            // the Bus projection (the datacenter poll seam).
            Plane::Fleet => menus.push(Menu::new(
                "Fleet",
                vec![Entry::Item(Item::new(
                    MenuAction::FleetRefresh,
                    "Refresh now",
                ))],
            )),
            // This Node / Network / Provisioning are read-only (`&self`) behind
            // this bar — no mutable verb seam exists, so no verb menu is
            // invented (§7 — honest omission, never a dead entry).
            Plane::ThisNode | Plane::Network | Plane::Provisioning => {}
        }

        menus.push(Menu::new(
            "Help",
            vec![Entry::Item(Item::new(
                MenuAction::HelpGuide,
                "Plane Guide\u{2026}",
            ))],
        ));
        menus
    }

    /// The live status cluster: the active plane, then — once the controller's
    /// snapshot has been parsed — the elected leader (or an honest "no leader")
    /// and the peer-directory count, and the fleet update target when the
    /// provisioning snapshot named one. Nothing pre-poll is fabricated (§7).
    fn build_status(
        active: Plane,
        controller: &ControllerState,
        provisioning: &ProvisioningState,
    ) -> Vec<StatusChip> {
        let mut chips = vec![StatusChip::new(active.label(), ChipTone::Info)];
        if controller.seen() {
            match controller.leader() {
                Some(leader) => chips.push(StatusChip::with_icon(
                    DOT,
                    format!("leader {leader}"),
                    ChipTone::Ok,
                )),
                None => chips.push(StatusChip::with_icon(DOT, "no leader", ChipTone::Warn)),
            }
            let peers = controller.peer_count();
            chips.push(StatusChip::new(
                format!("{peers} peer{}", if peers == 1 { "" } else { "s" }),
                ChipTone::Neutral,
            ));
        }
        if let Some(target) = provisioning.fleet_target() {
            chips.push(StatusChip::new(
                format!("target {target}"),
                ChipTone::Neutral,
            ));
        }
        chips
    }

    #[cfg(test)]
    mod tests {
        use super::{build_menus, build_status, CloudMenuVerb, MenuAction, Plane};
        use crate::controller::ControllerState;
        use crate::provisioning::ProvisioningState;
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

        /// Flatten a menu's item ids.
        fn ids(menu: &mde_egui::menubar::Menu<MenuAction>) -> Vec<MenuAction> {
            menu.entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i.id),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn view_menu_lists_every_plane_with_the_active_one_checked() {
            let menus = build_menus(Plane::Network);
            let view = &menus[0];
            assert_eq!(view.title, "View");
            let planes: Vec<Plane> = view
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => match i.id {
                        MenuAction::Plane(p) => Some(p),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(
                planes,
                Plane::ALL.to_vec(),
                "every plane is reachable, in order"
            );
            // Exactly the active plane is checked (radio) — the rest are
            // unchecked, never omitted (§7).
            for entry in &view.entries {
                if let Entry::Item(item) = entry {
                    assert_eq!(
                        item.checked,
                        Some(item.id == MenuAction::Plane(Plane::Network)),
                        "{:?} check-state must track the active plane",
                        item.id
                    );
                }
            }
        }

        #[test]
        fn verb_menus_appear_only_at_plane_depth() {
            // Cloud active → View · Cloud · Help, with every QC-12 verb.
            let menus = build_menus(Plane::Cloud);
            assert_eq!(menus.len(), 3);
            assert_eq!(menus[1].title, "Cloud");
            let cloud = ids(&menus[1]);
            for verb in [
                CloudMenuVerb::Refresh,
                CloudMenuVerb::LaunchInstance,
                CloudMenuVerb::NewVolume,
                CloudMenuVerb::RegisterImage,
                CloudMenuVerb::NewNetwork,
            ] {
                assert!(
                    cloud.contains(&MenuAction::Cloud(verb)),
                    "{verb:?} missing from the Cloud menu"
                );
            }

            // Fleet active → View · Fleet · Help, with the refresh verb.
            let menus = build_menus(Plane::Fleet);
            assert_eq!(menus.len(), 3);
            assert_eq!(menus[1].title, "Fleet");
            assert_eq!(ids(&menus[1]), vec![MenuAction::FleetRefresh]);

            // A read-only plane → View · Help only (honest omission, §7).
            for plane in [Plane::ThisNode, Plane::Network, Plane::Provisioning] {
                let menus = build_menus(plane);
                assert_eq!(menus.len(), 2, "{plane:?} must carry no verb menu");
                assert_eq!(menus[0].title, "View");
                assert_eq!(menus[1].title, "Help");
            }
        }

        #[test]
        fn help_carries_the_plane_guide() {
            let menus = build_menus(Plane::ThisNode);
            let help = menus.last().expect("a Help menu");
            assert_eq!(help.title, "Help");
            assert_eq!(ids(help), vec![MenuAction::HelpGuide]);
        }

        #[test]
        fn status_names_the_active_plane_and_omits_unpolled_facts() {
            // Pre-poll defaults: no snapshot parsed, no fleet target — only the
            // active-plane chip shows (nothing fabricated, §7).
            let controller = ControllerState::default();
            let provisioning = ProvisioningState::default();
            let chips = build_status(Plane::Provisioning, &controller, &provisioning);
            assert!(chips
                .iter()
                .any(|c| c.text == "Provisioning" && c.tone == ChipTone::Info));
            assert_eq!(
                chips.len(),
                1,
                "unpolled leader/peers/target chips must be omitted, not zeroed"
            );
        }

        #[test]
        fn menu_bar_renders_headless() {
            use mde_egui::egui::{self, pos2, vec2, Rect};
            use mde_egui::Style;
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let controller = ControllerState::default();
            let provisioning = ProvisioningState::default();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = super::show(ui, Plane::Cloud, &controller, &provisioning);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(
                !prims.is_empty(),
                "the State of the Mesh bar produced no primitives"
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
