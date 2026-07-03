//! The shell **dock** — the surface launcher rail beside the Workbench (E12-3b).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a compact vertical rail that selects which surface fills the
//! shell body — the mesh-control [`Workbench`](Surface::Workbench) (This Node →
//! Fleet, MV-6), this node's local VM [`Instances`](Surface::Instances) (the
//! cloud-hypervisor broker, E12-7), the brokered VM [`Desktop`](Surface::Desktop)
//! (VDI, egui-native), the three embedded app surfaces (Music / Files / Voice),
//! plus the unified [`Chat`](Surface::Chat) surface — the ONE notification
//! interface (ICQ roster + folded alerts + clipboard clips, NOTIFY-CHAT). One
//! surface shows at a time; the Workbench is always one click away.
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
    /// The live **Mesh Map** — the egui reincarnation of MESHMAP (`mde-mesh-view`):
    /// a procedural canvas of the current mesh (nodes by role + health, the elected
    /// leader, and the links between them), folded from the same world-readable
    /// mesh-status snapshot the Workbench planes read. An all-green onboard
    /// self-test auto-opens it (OW-10).
    MeshView,
    /// The VDI **Desktop** surface — a brokered VM desktop rendered egui-native
    /// (`mde-vdi-rdp` / `mde-vdi-vnc`), the point of E12 "Quasar".
    Desktop,
    /// The **Instances** surface — this workstation's local cloud-hypervisor VMs
    /// (`mde-kvm`): the create / boot / shutdown lifecycle broker (E12-7).
    Instances,
    /// The embedded Music surface (`mde-music-egui`).
    Music,
    /// The embedded Media surface (`mde-media-egui`) — the full media player
    /// (Sources / Library / Player / Queue) over the real `mde_media_core`
    /// backend (MEDIA-18).
    Media,
    /// The embedded Files surface (`mde-files-egui`).
    Files,
    /// The embedded Voice / SIP surface (`mde-voice-egui`).
    Voice,
    /// The Browser surface — the sandboxed Servo browser (`mde-web-preview`)
    /// rendered egui-native over the BOOKMARKS-6 IPC + shm texture bridge.
    Browser,
    /// The embedded Terminal surface (`mde-term-egui`) — the full Terminator-class
    /// terminal (tabs / splits / broadcast / a shell on any mesh peer, TERM-4/5/8)
    /// over a real local PTY, mounted as an in-shell panel (TERM-16).
    Terminal,
    /// The Chat surface — the ONE unified notification interface (NOTIFY-CHAT):
    /// every mesh host is a contact, and its alerts + clipboard copies are its
    /// messages, over the `state/chat/roster` + `state/chat/conversation/<key>`
    /// worker read-model. Subsumes the retired standalone Notifications +
    /// Clipboard surfaces (NOTIFY-CHAT-6 cutover).
    Chat,
    /// The System surface — this seat's host controls (audio mixer, Bluetooth,
    /// displays, power & battery, backlight, hotkeys), folded from `mde-seat`
    /// (E12-15). Owns ALL host-control interaction (lock 3); the chrome bar keeps
    /// only read-only status icons.
    System,
    /// The Storage surface — GParted-authentic disk/partition management (E12-21),
    /// folded from `state/storage/<node>` and driven back via `action/storage/<node>`.
    /// Segment bars + partition tables + a typed-armed pending-op queue, for this
    /// node and any mesh peer; the `mackesd` storage worker owns the walls + executor.
    Storage,
}

// This nav enum spells its variants `Surface::Music` rather than `Self::Music` on
// purpose: the explicit type keeps the `ALL` table, the labels, and the hints
// scannable side by side (a launcher reads clearer than a wall of `Self::`). Opt the
// block out of `clippy::use_self` rather than thread `Self::` through every arm.
#[allow(clippy::use_self)]
impl Surface {
    /// The dock entries in nav order — the Workbench (mesh-control home) first,
    /// then the live Mesh Map, then the local VM Instances broker + the brokered
    /// Desktop, then the three app surfaces, then the unified Chat surface (the ONE
    /// notification interface), and finally this seat's host-controls System +
    /// Storage surfaces.
    pub(crate) const ALL: [Surface; 13] = [
        Surface::Workbench,
        Surface::MeshView,
        Surface::Instances,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Voice,
        Surface::Browser,
        Surface::Terminal,
        Surface::Chat,
        Surface::System,
        Surface::Storage,
    ];

    /// The short dock label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::Workbench => "Workbench",
            Surface::MeshView => "Mesh Map",
            Surface::Instances => "Instances",
            Surface::Desktop => "Desktop",
            Surface::Music => "Music",
            Surface::Media => "Media",
            Surface::Files => "Files",
            Surface::Voice => "Voice",
            Surface::Browser => "Browser",
            Surface::Terminal => "Terminal",
            Surface::Chat => "Chat",
            Surface::System => "System",
            Surface::Storage => "Storage",
        }
    }

    /// A one-line hover hint — honest description of what the surface does, never a
    /// stand-in for live data (§7).
    pub(crate) const fn hint(self) -> &'static str {
        match self {
            Surface::Workbench => {
                "Mesh control — This Node, Controller, Network, Fleet, Provisioning."
            }
            Surface::MeshView => {
                "The live mesh map — nodes by role and health, the elected leader, and the links between them."
            }
            Surface::Instances => {
                "Manage this node's local VMs (cloud-hypervisor) — create, boot, shut down."
            }
            Surface::Desktop => {
                "Pick a discovered desktop (mesh peers, LAN, local VMs) and view it in-shell."
            }
            Surface::Music => "Play the mesh music library (Subsonic / Airsonic).",
            Surface::Media => {
                "Play local, Jellyfin & mesh media — Sources, Library, Player, Queue."
            }
            Surface::Files => "Browse local + peer folders and Send-To across the mesh.",
            Surface::Voice => "Place and receive mesh voice calls (SIP).",
            Surface::Browser => {
                "Browse the web in a sandboxed Servo browser rendered here in the shell."
            }
            Surface::Terminal => {
                "Open a shell — tabs, splits, broadcast input, and a shell on any mesh peer."
            }
            Surface::Chat => {
                "Mesh chat (ICQ) — every host is a contact; its alerts + clipboard copies are its messages."
            }
            Surface::System => {
                "This seat's host controls — audio mixer, Bluetooth, displays, power, hotkeys."
            }
            Surface::Storage => {
                "Disks & partitions across the mesh — stage a queue, arm the target, apply."
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
        // Thirteen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
        // two VM surfaces (Instances / Desktop), the app surfaces (Music / Media — the
        // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
        // browser, BOOKMARKS-6 / Terminal — the Terminator-class terminal over a real
        // PTY, TERM-16), the unified Chat surface (the ONE notification interface — the
        // standalone Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the
        // host-controls System surface, and the Storage surface (GParted-authentic disk
        // mgmt, E12-21).
        assert_eq!(Surface::ALL.len(), 13);
        assert_eq!(Surface::ALL[0], Surface::Workbench);
        for s in [
            Surface::MeshView,
            Surface::Instances,
            Surface::Desktop,
            Surface::Music,
            Surface::Media,
            Surface::Files,
            Surface::Voice,
            Surface::Browser,
            Surface::Terminal,
            Surface::Chat,
            Surface::System,
            Surface::Storage,
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
