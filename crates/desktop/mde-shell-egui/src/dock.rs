//! The shell **dock** — the surface launcher rail beside the Workbench (E12-3b).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a compact vertical rail that selects which surface fills the
//! shell body — the mesh-control [`Workbench`](Surface::Workbench) (This Node →
//! Fleet, MV-6), this node's local VM [`Instances`](Surface::Instances) (the
//! cloud-hypervisor broker, E12-7), the brokered VM [`Desktop`](Surface::Desktop)
//! (VDI, egui-native), the three embedded app surfaces (Music / Files / Voice),
//! plus the two mesh information surfaces (Notifications / Clipboard). One surface
//! shows at a time; the Workbench is always one click away.
//!
//! The rail is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.

use mde_egui::egui::{self, RichText};
use mde_egui::Style;

/// Which surface fills the shell body when the chrome bar is expanded.
///
/// [`Workbench`](Self::Workbench) is the default: expanding opens the mesh-control
/// Workbench exactly as it did before E12-3b — the three app surfaces are the
/// panels this unit adds beside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Surface {
    /// The five-plane mesh-control Workbench (This Node → Fleet).
    #[default]
    Workbench,
    /// The VDI **Desktop** surface — a brokered VM desktop rendered egui-native
    /// (`mde-vdi-rdp` / `mde-vdi-vnc`), the point of E12 "Quasar".
    Desktop,
    /// The **Instances** surface — this workstation's local cloud-hypervisor VMs
    /// (`mde-kvm`): the create / boot / shutdown lifecycle broker (E12-7).
    Instances,
    /// The embedded Music surface (`mde-music-egui`).
    Music,
    /// The embedded Files surface (`mde-files-egui`).
    Files,
    /// The embedded Voice / SIP surface (`mde-voice-egui`).
    Voice,
    /// The Notifications surface — mesh-wide alerts (security, presence,
    /// firewall, compute) tailed from the Bus alert lanes.
    Notifications,
    /// The Clipboard surface — recent mesh clipboard entries from
    /// `event/clipboard/clip`, newest first.
    Clipboard,
}

impl Surface {
    /// The dock entries in nav order — the Workbench (mesh-control home) first,
    /// then the local VM Instances broker + the brokered Desktop, then the three
    /// app surfaces, then the two mesh information surfaces.
    pub(crate) const ALL: [Surface; 8] = [
        Surface::Workbench,
        Surface::Instances,
        Surface::Desktop,
        Surface::Music,
        Surface::Files,
        Surface::Voice,
        Surface::Notifications,
        Surface::Clipboard,
    ];

    /// The short dock label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::Workbench => "Workbench",
            Surface::Instances => "Instances",
            Surface::Desktop => "Desktop",
            Surface::Music => "Music",
            Surface::Files => "Files",
            Surface::Voice => "Voice",
            Surface::Notifications => "Alerts",
            Surface::Clipboard => "Clipboard",
        }
    }

    /// A one-line hover hint — honest description of what the surface does, never a
    /// stand-in for live data (§7).
    pub(crate) const fn hint(self) -> &'static str {
        match self {
            Surface::Workbench => {
                "Mesh control — This Node, Controller, Network, Fleet, Provisioning."
            }
            Surface::Instances => {
                "Manage this node's local VMs (cloud-hypervisor) — create, boot, shut down."
            }
            Surface::Desktop => "View a brokered VM desktop (RDP / VNC), rendered in-shell.",
            Surface::Music => "Play the mesh music library (Subsonic / Airsonic).",
            Surface::Files => "Browse local + peer folders and Send-To across the mesh.",
            Surface::Voice => "Place and receive mesh voice calls (SIP).",
            Surface::Notifications => {
                "Mesh-wide alerts — security, presence, firewall, compute events."
            }
            Surface::Clipboard => {
                "Recent clipboard copies from across the mesh, newest first."
            }
        }
    }
}

/// Render the dock rail into `ui`, selecting the active [`Surface`]. A click on a
/// launcher makes that surface active; the active one reads as selected.
pub(crate) fn rail(ui: &mut egui::Ui, active: &mut Surface) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("SURFACES")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    let width = ui.available_width();
    for surface in Surface::ALL {
        if ui
            .add_sized(
                [width, Style::SP_L],
                egui::SelectableLabel::new(*active == surface, surface.label()),
            )
            .on_hover_text(surface.hint())
            .clicked()
        {
            *active = surface;
        }
        ui.add_space(Style::SP_XS);
    }
}

#[cfg(test)]
mod tests {
    use super::Surface;

    #[test]
    fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
        // Eight entries: Workbench first, two VM surfaces (Instances / Desktop),
        // three app surfaces (Music / Files / Voice), two info surfaces
        // (Notifications / Clipboard).
        assert_eq!(Surface::ALL.len(), 8);
        assert_eq!(Surface::ALL[0], Surface::Workbench);
        for s in [
            Surface::Instances,
            Surface::Desktop,
            Surface::Music,
            Surface::Files,
            Surface::Voice,
            Surface::Notifications,
            Surface::Clipboard,
        ] {
            assert!(Surface::ALL.contains(&s), "{s:?} missing from the dock");
        }
    }

    #[test]
    fn labels_and_hints_are_present_and_distinct() {
        for s in Surface::ALL {
            assert!(!s.label().is_empty(), "{s:?} has an empty label");
            // A hint is real descriptive copy, longer than its one-word label.
            assert!(s.hint().len() > s.label().len(), "{s:?} hint too short");
        }
        let mut labels: Vec<&str> = Surface::ALL.iter().map(|s| s.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            Surface::ALL.len(),
            "dock labels must be distinct"
        );
    }

    #[test]
    fn the_shell_opens_on_the_workbench_surface() {
        assert_eq!(Surface::default(), Surface::Workbench);
    }
}
