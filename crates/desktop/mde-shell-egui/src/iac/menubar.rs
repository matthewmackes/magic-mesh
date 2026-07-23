//! MENUBAR-ALL (Workloads) — the shared bar over the delivery-type cockpit. Every
//! item is a real seam (§6): the **Cloud** spine (Refresh), the **Provision** +
//! **Configure** action menus (the plan/apply gate as mouse twins of the body's
//! own buttons), a **View** menu that jumps to any of the five delivery views, a
//! **Panels** menu that opens any lens, and **Help**. The status cluster reads the
//! live `state/cloud` fold (nodes · backend-ready · apply posture). Every entry
//! maps to a landed seam; nothing is a dead entry (§8).

use super::{DeliveryView, Panel, WorkloadsState, CLOUD_PRODUCT_LABEL};
use mde_egui::egui::Ui;
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

/// One menu action — each routes to a real workspace seam in [`apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuAction {
    /// Force an immediate re-fold of the `state/cloud` mirror (`Cloud → Refresh`).
    Refresh,
    /// Jump to a delivery view (`View → <view>`) — snaps the lens to its roster.
    Goto(DeliveryView),
    /// Open a lens (`Panels → <panel>`).
    Open(Panel),
    /// Emit a provision plan (dry-run) — direct (`Provision → Plan`).
    ProvisionPlan,
    /// Open the typed-confirm for a live provision apply (`Provision → Apply`).
    ProvisionApply,
    /// Emit a configuration check (dry-run) — direct (`Configure → Check`).
    ConfigureCheck,
    /// Open the typed-confirm for a live configuration apply (`Configure → Apply`).
    ConfigureApply,
    /// Help → surface the apply-gate + audit posture in the action note.
    HelpAbout,
}

/// Render the WORKLOADS bar and return the action picked this frame.
pub(super) fn show(ui: &mut Ui, state: &WorkloadsState) -> Option<MenuAction> {
    let menus = build_menus();
    let status = build_status(state);
    let model = MenuBarModel {
        // The dock groups Workloads under its purple categorical accent (§4).
        title: super::WORKSPACE_TITLE,
        accent: Style::ACCENT_WORKLOADS,
        menus: &menus,
        status: &status,
    };
    MenuBar::show(ui, &model)
}

/// Build the Cloud / Provision / Configure / View / Panels / Help menus — every
/// item a real landed seam (§8).
fn build_menus() -> Vec<Menu<MenuAction>> {
    vec![
        Menu::new(
            "Cloud",
            vec![Entry::Item(Item::new(
                MenuAction::Refresh,
                "Refresh mirror",
            ))],
        ),
        Menu::new(
            "Provision",
            vec![
                Entry::Item(Item::new(MenuAction::ProvisionPlan, "Plan (dry-run)")),
                Entry::Item(Item::new(
                    MenuAction::ProvisionApply,
                    "Apply infrastructure\u{2026}",
                )),
            ],
        ),
        Menu::new(
            "Configure",
            vec![
                Entry::Item(Item::new(MenuAction::ConfigureCheck, "Check (dry-run)")),
                Entry::Item(Item::new(
                    MenuAction::ConfigureApply,
                    "Apply configuration\u{2026}",
                )),
            ],
        ),
        build_view_menu(),
        build_panels_menu(),
        build_help_menu(),
    ]
}

/// The **View** menu — one jump per delivery view (the mouse twin of the
/// delivery-view selector).
fn build_view_menu() -> Menu<MenuAction> {
    Menu::new(
        "View",
        DeliveryView::ALL
            .iter()
            .map(|view| Entry::Item(Item::new(MenuAction::Goto(*view), view.label())))
            .collect(),
    )
}

/// The **Panels** menu — one jump per lens (the mouse twin of the lens sub-nav).
fn build_panels_menu() -> Menu<MenuAction> {
    Menu::new(
        "Panels",
        Panel::ALL
            .iter()
            .map(|panel| Entry::Item(Item::new(MenuAction::Open(*panel), panel.label())))
            .collect(),
    )
}

/// The **Help** menu — an honest surface identity caption + a real seam (the
/// apply-gate + audit posture note), so even Help carries no dead entry (§8).
fn build_help_menu() -> Menu<MenuAction> {
    Menu::new(
        "Help",
        vec![
            Entry::Caption(format!(
                "Workloads \u{2014} the {CLOUD_PRODUCT_LABEL} delivery-type cockpit (OpenTofu + \
                 Ansible + libvirt + Podman)."
            )),
            Entry::Item(Item::new(
                MenuAction::HelpAbout,
                "Apply gate + audit posture\u{2026}",
            )),
        ],
    )
}

/// The live status cluster — nodes · backend-ready · apply posture, folded from
/// the `state/cloud` mirror (§7 — honest, never a placeholder).
fn build_status(state: &WorkloadsState) -> Vec<StatusChip> {
    let states = state.states();
    if states.is_empty() {
        return vec![StatusChip::new("no cloud mirror", ChipTone::Warn)];
    }
    let total = states.len();
    let ready = states.iter().filter(|s| s.backend_ready()).count();
    let armed = states.iter().any(|s| s.apply_armed);
    vec![
        StatusChip::new(
            format!("{total} node{}", if total == 1 { "" } else { "s" }),
            ChipTone::Neutral,
        ),
        StatusChip::new(
            format!("{ready}/{total} ready"),
            if ready == total {
                ChipTone::Ok
            } else {
                ChipTone::Warn
            },
        ),
        StatusChip::new(
            if armed { "live-armed" } else { "plan-only" },
            if armed {
                ChipTone::Danger
            } else {
                ChipTone::Ok
            },
        ),
    ]
}

/// Apply a picked action to its real seam (§6) — each the exact affordance the
/// body's own control drives.
pub(super) fn apply(state: &mut WorkloadsState, action: MenuAction) {
    match action {
        MenuAction::Refresh => state.request_refresh(),
        MenuAction::Goto(view) => {
            state.set_view(view);
            state.set_panel(Panel::Roster);
        }
        MenuAction::Open(panel) => state.set_panel(panel),
        MenuAction::ProvisionPlan => {
            state.set_panel(Panel::Provision);
            state.plan_provision();
        }
        MenuAction::ProvisionApply => {
            state.set_panel(Panel::Provision);
            state.arm_provision();
        }
        MenuAction::ConfigureCheck => {
            state.set_panel(Panel::Configure);
            state.check_configure();
        }
        MenuAction::ConfigureApply => {
            state.set_panel(Panel::Configure);
            state.arm_configure();
        }
        MenuAction::HelpAbout => state.set_help_note(),
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::super::{DeliveryView, Panel, WorkloadsState};
    use super::{apply, build_help_menu, build_menus, build_status, MenuAction};
    use mde_egui::menubar::{Entry, Menu};
    use mde_egui::ChipTone;

    /// The menu titled `title`, if the generator built one.
    fn menu<'a>(menus: &'a [Menu<MenuAction>], title: &str) -> Option<&'a Menu<MenuAction>> {
        menus.iter().find(|m| m.title == title)
    }

    /// The item ids of a menu, in order.
    fn ids(menu: &Menu<MenuAction>) -> Vec<MenuAction> {
        menu.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(i.id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_bar_carries_the_plan_apply_gate_and_every_view_and_panel_jump() {
        let menus = build_menus();
        // The plan/apply gate lives in the Provision menu.
        let provision = menu(&menus, "Provision").expect("Provision menu");
        for want in [MenuAction::ProvisionPlan, MenuAction::ProvisionApply] {
            assert!(ids(provision).contains(&want), "missing {want:?}");
        }
        // Configure carries check + apply.
        let configure = menu(&menus, "Configure").expect("Configure menu");
        assert!(ids(configure).contains(&MenuAction::ConfigureCheck));
        assert!(ids(configure).contains(&MenuAction::ConfigureApply));
        // View jumps to every delivery view.
        let view = menu(&menus, "View").expect("View menu");
        assert_eq!(ids(view).len(), DeliveryView::ALL.len());
        for v in DeliveryView::ALL {
            assert!(ids(view).contains(&MenuAction::Goto(v)), "missing {v:?}");
        }
        // Panels opens every lens.
        let panels = menu(&menus, "Panels").expect("Panels menu");
        assert_eq!(ids(panels).len(), Panel::ALL.len());
        for p in Panel::ALL {
            assert!(ids(panels).contains(&MenuAction::Open(p)), "missing {p:?}");
        }
    }

    #[test]
    fn apply_drives_the_real_seams() {
        let mut state = WorkloadsState::default();
        // A view jump switches the view + snaps the lens to the roster.
        apply(&mut state, MenuAction::Goto(DeliveryView::AndroidVm));
        assert_eq!(state.view(), DeliveryView::AndroidVm);
        assert_eq!(state.panel(), Panel::Roster);
        // A panel jump opens the lens.
        apply(&mut state, MenuAction::Open(Panel::Status));
        assert_eq!(state.panel(), Panel::Status);
        // Apply infrastructure opens the typed-confirm (nothing publishes yet).
        apply(&mut state, MenuAction::ProvisionApply);
        assert_eq!(state.panel(), Panel::Provision);
        assert!(state.has_arming(), "Apply opens the typed-confirm");
        // Help surfaces the honest posture note.
        apply(&mut state, MenuAction::HelpAbout);
        assert!(state.note_text().is_some_and(|n| n.contains("apply")));
    }

    #[test]
    fn the_help_caption_stays_provider_neutral() {
        let help = build_help_menu();
        assert_eq!(help.title, "Help");
        let caption = help
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Caption(t) => Some(t.as_str()),
                _ => None,
            })
            .expect("help caption");
        for backend in ["OpenStack", "Nova", "Heat", "Keystone"] {
            assert!(!caption.contains(backend), "leaked backend term: {caption}");
        }
    }

    #[test]
    fn status_reads_the_mirror_fold_honestly() {
        // No mirror → an honest "no cloud mirror" warn chip.
        let empty = WorkloadsState::default();
        assert!(build_status(&empty)
            .iter()
            .any(|c| c.text == "no cloud mirror" && c.tone == ChipTone::Warn));
    }
}
