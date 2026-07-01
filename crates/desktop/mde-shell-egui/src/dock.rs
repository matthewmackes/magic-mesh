//! The shell **dock** — the surface launcher rail beside the Workbench (E12-3b).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a compact vertical rail that selects which surface fills the
//! shell body — the mesh-control [`Workbench`](Surface::Workbench) (This Node →
//! Fleet, MV-6) or one of the three embedded app surfaces (Music / Files / Voice).
//! One surface shows at a time; the Workbench is always one click away.
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
    /// The embedded Music surface (`mde-music-egui`).
    Music,
    /// The embedded Files surface (`mde-files-egui`).
    Files,
    /// The embedded Voice / SIP surface (`mde-voice-egui`).
    Voice,
}

impl Surface {
    /// The dock entries in nav order — the Workbench (mesh-control home) first,
    /// then the three app surfaces.
    pub(crate) const ALL: [Surface; 4] = [
        Surface::Workbench,
        Surface::Music,
        Surface::Files,
        Surface::Voice,
    ];

    /// The short dock label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::Workbench => "Workbench",
            Surface::Music => "Music",
            Surface::Files => "Files",
            Surface::Voice => "Voice",
        }
    }

    /// A one-line hover hint — honest description of what the surface does, never a
    /// stand-in for live data (§7).
    pub(crate) const fn hint(self) -> &'static str {
        match self {
            Surface::Workbench => {
                "Mesh control — This Node, Controller, Network, Fleet, Provisioning."
            }
            Surface::Music => "Play the mesh music library (Subsonic / Airsonic).",
            Surface::Files => "Browse local + peer folders and Send-To across the mesh.",
            Surface::Voice => "Place and receive mesh voice calls (SIP).",
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
    fn the_dock_lists_the_workbench_plus_the_three_app_surfaces() {
        // Exactly four entries, Workbench first, the three embedded surfaces after.
        assert_eq!(Surface::ALL.len(), 4);
        assert_eq!(Surface::ALL[0], Surface::Workbench);
        for s in [Surface::Music, Surface::Files, Surface::Voice] {
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
        assert_eq!(labels.len(), 4, "dock labels must be distinct");
    }

    #[test]
    fn the_shell_opens_on_the_workbench_surface() {
        assert_eq!(Surface::default(), Surface::Workbench);
    }
}
