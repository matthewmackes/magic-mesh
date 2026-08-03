//! Active lifecycle-first projection of the governed AOSP starter catalog.
//!
//! This panel deliberately renders pending contract state only. The shell has
//! no live Android guest inventory or launch dispatcher yet, so none of the
//! typed launcher intents exposed by the shared contract are actionable here.

use mackes_mesh_types::android_apps::{pending_starter_entries, AndroidAppInventoryEntry};
use mde_egui::egui::{self, RichText};
use mde_egui::{card, inset, muted_note, status_dot, Style};

/// Render the governed starter-image expectations for the Android VM filter.
pub(super) fn catalog_panel(ui: &mut egui::Ui, vm_scopes: &[String]) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("AOSP starter apps")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    muted_note(
        ui,
        format!(
            "{} The nine governed identities below are starter-image expectations, not live package evidence. Guest inventory and typed launch dispatch remain pending.",
            scope_text(vm_scopes)
        ),
    );
    ui.add_space(Style::SP_XS);

    let entries = pending_starter_entries();
    card().show(ui, |ui| {
        for (index, entry) in entries.iter().enumerate() {
            starter_app_row(ui, entry);
            if index + 1 < entries.len() {
                ui.separator();
            }
        }
    });
    ui.add_space(Style::SP_S);
}

/// Describe exactly which reported outer Android VMs this pending projection
/// applies to. Scope names include placement, and the caller sorts/deduplicates
/// them before rendering so the mirror remains deterministic.
fn scope_text(vm_scopes: &[String]) -> String {
    match vm_scopes {
        [] => "No Android VM is reporting guest inventory yet.".to_owned(),
        [scope] => format!(
            "Scoped to Android VM {scope}; its per-VM guest app-inventory mirror is not connected yet."
        ),
        scopes => format!(
            "Scoped across {} reported Android VMs ({}); their per-VM guest app-inventory mirrors are not connected yet.",
            scopes.len(),
            scopes.join(", ")
        ),
    }
}

/// Render one immutable identity plus all three pending evidence surfaces. The
/// disabled button has no click handler: a typed intent is not launch plumbing.
fn starter_app_row(ui: &mut egui::Ui, entry: &AndroidAppInventoryEntry) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(entry.descriptor.app.display_name())
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT),
        );
        category_tag(ui, entry.descriptor.category.label());
        ui.label(
            RichText::new(entry.descriptor.package_id.as_str())
                .size(Style::SMALL)
                .monospace()
                .color(Style::TEXT_DIM),
        );
        status_dot(ui, Style::TEXT_DIM);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(entry.availability.label()).size(Style::SMALL),
        );
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(entry.readiness.label()).size(Style::SMALL),
        );
        ui.add_enabled(
            false,
            egui::Button::new(RichText::new(entry.launch_readiness.label()).size(Style::SMALL)),
        )
        .on_hover_text(
            "The MAIN + LAUNCHER intent is typed, but no live guest dispatch path is available.",
        );
    });
}

fn category_tag(ui: &mut egui::Ui, label: &str) {
    inset().show(ui, |ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::android_apps::{
        AndroidAppAvailability, AndroidAppReadiness, AndroidLaunchReadiness, AOSP_STARTER_APP_COUNT,
    };

    #[test]
    fn governed_catalog_is_nine_pending_non_launchable_entries() {
        let entries = pending_starter_entries();
        assert_eq!(entries.len(), AOSP_STARTER_APP_COUNT);
        assert!(entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                && entry.readiness == AndroidAppReadiness::GuestPending
                && entry.launch_readiness == AndroidLaunchReadiness::IntegrationPending
                && !entry.is_launchable()
        }));
    }

    #[test]
    fn scope_copy_names_zero_one_and_multiple_android_vms() {
        assert_eq!(
            scope_text(&[]),
            "No Android VM is reporting guest inventory yet."
        );
        assert_eq!(
            scope_text(&["android-1 on eagle".to_owned()]),
            "Scoped to Android VM android-1 on eagle; its per-VM guest app-inventory mirror is not connected yet."
        );
        assert_eq!(
            scope_text(&[
                "android-1 on eagle".to_owned(),
                "android-2 on falcon".to_owned(),
            ]),
            "Scoped across 2 reported Android VMs (android-1 on eagle, android-2 on falcon); their per-VM guest app-inventory mirrors are not connected yet."
        );
    }

    #[test]
    fn lifecycle_gate_is_android_plan_and_run_only() {
        use super::super::{should_show_android_starter_catalog, DeliveryView, ResourceTableMode};

        assert!(should_show_android_starter_catalog(
            ResourceTableMode::Plan,
            DeliveryView::AndroidVm
        ));
        assert!(should_show_android_starter_catalog(
            ResourceTableMode::Run,
            DeliveryView::AndroidVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Drift,
            DeliveryView::AndroidVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Plan,
            DeliveryView::DesktopVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Containers,
            DeliveryView::ServiceContainer
        ));
    }
}
