//! WL-FUNC-023 S17 leftover — Status/self-test unit catalog for the wizard.
//!
//! [`crate::setup_action::wizard_services`] still reports the enable/mask set
//! from [`units_for_role`], which lists monolithic `mackesd.service` on every
//! rank. Status and post-Create/Join self-test must match the live plane the
//! same way first-boot does: grouped `mackesd-*.service` when that RPM is
//! installed, no first-boot oneshot, no workstation etcd member.

use crate::setup_action::SetupRole;
use mackesd_core::onboard::firstboot::runtime_expected_units;
use mackesd_core::onboard::role_provision::GROUPED_MACKESD_CONTROL_UNIT_FILE;
use std::path::Path;

/// True when the shipped RPM installed the grouped control-plane unit file.
#[must_use]
pub fn grouped_plane_installed() -> bool {
    Path::new(GROUPED_MACKESD_CONTROL_UNIT_FILE).is_file()
}

/// Units the wizard Status and self-test screens report as the live plane.
///
/// `grouped_plane` is injected so tests can plant both RPM shapes without
/// touching systemd. Production passes [`grouped_plane_installed`].
#[must_use]
pub fn status_units(role: SetupRole, grouped_plane: bool) -> Vec<String> {
    let role = match role {
        SetupRole::Lighthouse => mde_role::Role::Lighthouse,
        SetupRole::Workstation => mde_role::Role::Workstation,
    };
    runtime_expected_units(role, grouped_plane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackesd_core::onboard::role_provision::{units_for_role, GROUPED_MACKESD_UNITS};

    const FIRSTBOOT_ONESHOT: &str = "mcnf-lifecycle-firstboot.service";

    #[test]
    fn grouped_status_catalog_refuses_monolithic_mackesd_as_the_only_plane() {
        // The enable/mask catalog still lists the retired unit. Status must
        // not copy that list onto a grouped-plane seat.
        let enable_mask = units_for_role(mde_role::Role::Workstation);
        assert!(
            enable_mask.contains(&"mackesd.service"),
            "units_for_role still names monolithic mackesd.service — Status must not treat it as the live plane"
        );
        assert!(
            !enable_mask.contains(&"mackesd-control.service"),
            "units_for_role must not silently grow a second catalog: {enable_mask:?}"
        );

        let workstation = status_units(SetupRole::Workstation, true);
        assert!(
            !workstation.iter().any(|unit| unit == "mackesd.service"),
            "grouped workstation Status must not require monolithic mackesd.service: {workstation:?}"
        );
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                workstation.iter().any(|active| active == unit),
                "grouped workstation Status must require {unit}: {workstation:?}"
            );
        }
        assert!(workstation.iter().any(|unit| unit == "nebula.service"));
        assert!(workstation
            .iter()
            .any(|unit| unit == "mde-shell-egui.service"));
        assert!(workstation
            .iter()
            .any(|unit| unit == "mcnf-node-virt.service"));
        assert!(
            !workstation.iter().any(|unit| unit == "etcd.service"),
            "grouped workstation Status must not require a workstation etcd member: {workstation:?}"
        );
        assert!(
            !workstation.iter().any(|unit| unit == FIRSTBOOT_ONESHOT),
            "Status must not require the activating first-boot oneshot: {workstation:?}"
        );
        assert!(
            !workstation.iter().any(|unit| unit.ends_with(".timer")),
            "grouped workstation Status must drop timer leaks: {workstation:?}"
        );
        assert!(
            !workstation
                .iter()
                .any(|unit| unit.contains("collaboration-identity")),
            "Status must not invent dest-gated collab-identity: {workstation:?}"
        );

        let lighthouse = status_units(SetupRole::Lighthouse, true);
        assert!(!lighthouse.iter().any(|unit| unit == "mackesd.service"));
        assert!(lighthouse.iter().any(|unit| unit == "etcd.service"));
        assert!(!lighthouse
            .iter()
            .any(|unit| unit == "mde-shell-egui.service"));
        assert!(!lighthouse
            .iter()
            .any(|unit| unit == "mcnf-node-virt.service"));
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                lighthouse.iter().any(|active| active == unit),
                "grouped lighthouse Status must require {unit}"
            );
        }

        let thin = status_units(SetupRole::Lighthouse, false);
        assert!(
            thin.iter().any(|unit| unit == "mackesd.service"),
            "thin lighthouse without the grouped unit file still reports monolithic mackesd.service"
        );
        assert!(!thin.iter().any(|unit| unit == "mackesd-control.service"));
    }
}
