//! Canonical Construct surface taxonomy and shared surface presentation helpers.
//!
//! This module is deliberately chrome-neutral: Springboard, Spotlight, the app
//! switcher, Car, and tests all consume the same surface order, grouping, labels,
//! shared icon-registry loader, and session-summary types.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};

/// Which surface fills the shell body.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Surface {
    /// Unified worker tree, node operations, network operations, and discovery
    /// interface. This is the only canonical node-management workspace.
    Workers,
    /// Legacy deep-link alias for the Workers control tab.
    FleetMesh,
    /// Legacy deep-link alias for the Workbench tab inside [`Surface::FleetMesh`].
    Workbench,
    /// Legacy deep-link alias for the Mesh Map tab inside [`Surface::FleetMesh`].
    MeshView,
    /// Legacy deep-link alias for the Explorer tab inside [`Surface::FleetMesh`].
    Explorer,
    /// Brokered remote desktop sessions.
    #[default]
    Desktop,
    /// Workloads / infrastructure-as-code control plane.
    InfraCode,
    /// Music library and player.
    Music,
    /// General media surface.
    Media,
    /// Files browser.
    Files,
    /// Sandboxed browser.
    Browser,
    /// Maps, location, and vehicle management.
    MapsLocation,
    /// Local and remote terminal.
    Terminal,
    /// Paired-phone management.
    Phones,
    /// Collaboration and communications hub.
    Communications,
    /// Legacy deep-link alias for the Workers local-node tab.
    ThisNode,
    /// Host settings and controls.
    System,
    /// Disk and partition management.
    Storage,
    /// Platform identity and legal information.
    About,
    /// Daemon-projected Clock workspace, reached from the status clock.
    Clock,
    /// Car-profile dashboard home.
    AutoHome,
}

#[allow(clippy::use_self)]
impl Surface {
    /// Every Springboard/Spotlight surface in canonical keyboard order.
    pub(crate) const ALL: [Surface; 10] = [
        Surface::Workers,
        Surface::InfraCode,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Browser,
        Surface::MapsLocation,
        Surface::Terminal,
        Surface::Communications,
    ];

    /// The shared icon-registry glyph for this surface.
    pub(crate) const fn icon_id(self) -> IconId {
        match self {
            Surface::Workers | Surface::ThisNode | Surface::System => IconId::Node,
            Surface::FleetMesh | Surface::Workbench | Surface::MeshView | Surface::Explorer => {
                IconId::MeshView
            }
            Surface::AutoHome => IconId::Workbench,
            Surface::InfraCode => IconId::Server,
            Surface::Desktop => IconId::Desktop,
            Surface::Music => IconId::Music,
            Surface::Media => IconId::Media,
            Surface::Files => IconId::Files,
            Surface::Browser => IconId::Browser,
            Surface::MapsLocation => IconId::MapsLocation,
            Surface::Terminal => IconId::Terminal,
            Surface::Phones => IconId::Phones,
            Surface::Communications => IconId::Teams,
            Surface::Storage => IconId::Storage,
            Surface::About | Surface::Clock => IconId::Mark,
        }
    }

    /// Human-facing label shared by every launcher and switcher.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::Workers => "Control Panel",
            Surface::FleetMesh | Surface::Workbench | Surface::MeshView | Surface::Explorer => {
                "Fleet & Mesh"
            }
            Surface::InfraCode => "Infra as Code",
            Surface::Desktop => "Remote Sessions",
            Surface::Music => "Music",
            Surface::Media => "Media",
            Surface::Files => "Files",
            Surface::Browser => "Browser",
            Surface::MapsLocation => "Maps & Location",
            Surface::Terminal => "Terminal",
            Surface::Phones => "Phones",
            Surface::Communications => "Mesh Teams",
            Surface::ThisNode | Surface::System | Surface::Storage | Surface::About => "This Node",
            Surface::Clock => "Clock",
            Surface::AutoHome => "Car Home",
        }
    }
}

/// Workspaces owned by the notification/tool tray. These remain fully
/// launchable through direct routes and keyboard shortcuts, but are not
/// duplicated in the central launcher taxonomy.
pub(crate) const TOOL_TRAY_SURFACES: [Surface; 4] = [
    Surface::Workers,
    Surface::Music,
    Surface::Media,
    Surface::Terminal,
];

#[must_use]
pub(crate) const fn is_tool_tray_surface(surface: Surface) -> bool {
    matches!(
        surface,
        Surface::Workers
            | Surface::FleetMesh
            | Surface::Music
            | Surface::Media
            | Surface::ThisNode
            | Surface::System
            | Surface::Storage
            | Surface::About
            | Surface::Terminal
    )
}

/// Collapse every historical node-management route to the single Workers
/// workspace. The legacy variants remain in the enum only so persisted
/// preferences, alerts, and external deep links can be migrated safely.
#[must_use]
pub(crate) const fn canonical_workspace_surface(surface: Surface) -> Surface {
    match surface {
        Surface::Workers
        | Surface::FleetMesh
        | Surface::Workbench
        | Surface::MeshView
        | Surface::Explorer
        | Surface::ThisNode
        | Surface::System
        | Surface::Storage
        | Surface::About => Surface::Workers,
        Surface::Files => Surface::Communications,
        Surface::Phones => Surface::Workers,
        surface => surface,
    }
}

/// Desktop egui crates embedded by the shell's launchable surface catalog.
///
/// This is intentionally a package-name list rather than a second navigation
/// model: the static RPM gate reads it to catch a crate that compiles in the
/// workspace but is no longer reachable from the shipped shell. The panel
/// client and the shell host are deliberately outside this list.
#[allow(dead_code)]
pub(crate) const EMBEDDED_SURFACE_CRATES: [&str; 8] = [
    "mde-bookmarks-egui",
    "mde-collab-egui",
    "mde-editor-egui",
    "mde-files-egui",
    "mde-maps-location-egui",
    "mde-media-egui",
    "mde-music-egui",
    "mde-term-egui",
];

/// Adoption state of one visual-system concern in the WL-UX-009 inventory.
///
/// This is an audit state rather than a product-health verdict: it makes
/// remaining migration visible without calling a surface production-ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VisualAdoption {
    /// The surface consumes the governed shared primitive for this concern.
    Adopted,
    /// The surface consumes part of the shared system; migration remains.
    Partial,
    /// The concern has no governed implementation yet.
    Gap,
    /// A documented rendering boundary changes the normal Construct rule.
    Exception,
}

/// Deliberate visual boundary for a launchable surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VisualBoundary {
    /// A normal Construct workspace must converge on shared primitives.
    Construct,
    /// A focused remote desktop preserves every guest pixel without shell chrome.
    FocusedVdiPixels,
    /// Map content may retain a content-specific palette.
    MapsContentColour,
    /// The Browser migration ends at the VM guest; guest Chromium is not Construct UI.
    BrowserVmGuest,
}

/// Complete visual-system classification for one launchable Construct surface.
///
/// The human-readable findings and migration order live in
/// `docs/design/platform-interfaces.md`; this table is the mechanically checked
/// companion tied to [`Surface::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceVisualInventory {
    /// Launchable surface being classified.
    pub(crate) surface: Surface,
    /// The only governed rendering-boundary exception, if any.
    pub(crate) boundary: VisualBoundary,
    /// Common app frame and chrome.
    pub(crate) app_frame: VisualAdoption,
    /// Navigation bar, sidebar, and route hierarchy.
    pub(crate) navigation: VisualAdoption,
    /// Loading, empty, stale, offline, error, and destructive states.
    pub(crate) states: VisualAdoption,
    /// Sheets, popovers, and destructive confirmations.
    pub(crate) dialogs: VisualAdoption,
    /// Themed hover and disabled-control help.
    pub(crate) tooltips: VisualAdoption,
    /// Shared registry glyphs and semantic icon treatment.
    pub(crate) icons: VisualAdoption,
    /// Centralized expressive motion and effects.
    pub(crate) motion: VisualAdoption,
    /// Dense operational table/list presentation.
    pub(crate) lists: VisualAdoption,
    /// Registry provenance and asset-license audit.
    pub(crate) licensing: VisualAdoption,
    /// Quazar Dark and Light appearance proof.
    pub(crate) dark_light: VisualAdoption,
}

const PARTIAL_CONSTRUCT_VISUAL: SurfaceVisualInventory = SurfaceVisualInventory {
    surface: Surface::Workers,
    boundary: VisualBoundary::Construct,
    // Every normal Construct surface now uses the shared workspace chrome. The
    // remaining Partial fields below are independent concerns (states,
    // dialogs, icons, motion, lists, and licensing).
    app_frame: VisualAdoption::Adopted,
    navigation: VisualAdoption::Partial,
    states: VisualAdoption::Partial,
    dialogs: VisualAdoption::Partial,
    // `lint-style-leaks.sh` mechanically rejects raw egui hover text.
    tooltips: VisualAdoption::Adopted,
    icons: VisualAdoption::Partial,
    motion: VisualAdoption::Partial,
    lists: VisualAdoption::Partial,
    licensing: VisualAdoption::Gap,
    dark_light: VisualAdoption::Partial,
};

/// The complete WL-UX-009 launchable-egui visual inventory.
///
/// `Partial` and `Gap` are evidence of remaining work, not permission to bypass
/// the shared system. Only the three named visual boundaries depart from the
/// normal Construct workspace rule.
#[allow(dead_code)]
pub(crate) const SURFACE_VISUAL_INVENTORY: [SurfaceVisualInventory; 10] = [
    PARTIAL_CONSTRUCT_VISUAL,
    SurfaceVisualInventory {
        surface: Surface::InfraCode,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Desktop,
        boundary: VisualBoundary::FocusedVdiPixels,
        app_frame: VisualAdoption::Exception,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Music,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Media,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Browser,
        boundary: VisualBoundary::BrowserVmGuest,
        // Construct owns the Browser VM connection, unavailable, and
        // diagnostic boundary; only the guest viewport stops at the VM edge.
        // Host controller/navigation adoption remains partial.
        app_frame: VisualAdoption::Adopted,
        navigation: VisualAdoption::Partial,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::MapsLocation,
        boundary: VisualBoundary::MapsContentColour,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Terminal,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Files,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
    SurfaceVisualInventory {
        surface: Surface::Communications,
        ..PARTIAL_CONSTRUCT_VISUAL
    },
];

/// One launcher taxonomy group used for color coding and Spotlight grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LauncherGroup {
    /// Human group title, used by Spotlight and other grouped launchers.
    pub(crate) label: &'static str,
    /// Shared group accent.
    pub(crate) accent: egui::Color32,
    /// Surfaces in this group in canonical order.
    pub(crate) surfaces: &'static [Surface],
}

/// The central launcher taxonomy. Tool-tray-owned workspaces are intentionally
/// omitted so the center launcher does not duplicate the notification tray;
/// the single Springboard desktop uses this flattened order.
pub(crate) const LAUNCHER_GROUPS: [LauncherGroup; 4] = [
    LauncherGroup {
        label: "Mesh Control",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::InfraCode],
    },
    LauncherGroup {
        label: "Desktop & Session",
        accent: Style::ACCENT,
        surfaces: &[Surface::Desktop, Surface::MapsLocation],
    },
    LauncherGroup {
        label: "Web",
        accent: Style::ACCENT_WEB,
        surfaces: &[Surface::Browser],
    },
    LauncherGroup {
        label: "Mesh Teams",
        accent: Style::ACCENT_TEAMS,
        surfaces: &[Surface::Communications],
    },
];

/// Compact Springboard Dock grouping selected by the operator survey.
///
/// This is intentionally narrower than [`LAUNCHER_GROUPS`]: the all-icons
/// desktop, Spotlight, and switcher retain the complete taxonomy, while the
/// always-present dock carries only the high-frequency app clusters. The
/// `Infra` pair is the operator's VM/remote-session launcher plus Terminal;
/// the broader Workloads control plane stays in the full catalog.
pub(crate) const DOCK_LAUNCHER_GROUPS: [LauncherGroup; 3] = [
    LauncherGroup {
        label: "Infra",
        accent: Style::ACCENT_TERMINALS,
        surfaces: &[Surface::Desktop, Surface::Terminal],
    },
    LauncherGroup {
        label: "Ops",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::MapsLocation, Surface::Communications],
    },
    LauncherGroup {
        label: "Life",
        accent: Style::ACCENT_MEDIA,
        surfaces: &[Surface::Music, Surface::Media, Surface::Browser],
    },
];

/// Surface at a flattened launcher tile position (group order, then row order).
#[must_use]
pub(crate) fn springboard_surface(index: usize) -> Option<Surface> {
    LAUNCHER_GROUPS
        .iter()
        .flat_map(|group| group.surfaces.iter().copied())
        .nth(index)
}

const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        let mut count = 0;
        let mut group = 0;
        while group < LAUNCHER_GROUPS.len() {
            let surfaces = LAUNCHER_GROUPS[group].surfaces;
            let mut surface = 0;
            while surface < surfaces.len() {
                if surfaces[surface] as usize == target {
                    count += 1;
                }
                surface += 1;
            }
            group += 1;
        }
        let expected =
            if is_tool_tray_surface(Surface::ALL[i]) || matches!(Surface::ALL[i], Surface::Files) {
                0
            } else {
                1
            };
        assert!(
            count == expected,
            "central launcher must contain non-tray surfaces once and tray-owned surfaces zero times",
        );
        i += 1;
    }
};

/// Dock group label for a high-frequency surface, or an empty string for
/// surfaces that remain launchable from the full Springboard/Spotlight surface.
pub(crate) fn dock_launcher_group_label(surface: Surface) -> &'static str {
    DOCK_LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map_or("", |group| group.label)
}

/// Dock-specific app label for compact operator wording.
///
/// The underlying surface labels stay stable for search, switcher, and deep-link
/// copy. The Dock can still present the survey terms: VMs for the Remote
/// Sessions launcher and File Manager for the Files surface.
pub(crate) const fn dock_launcher_surface_label(surface: Surface) -> &'static str {
    match surface {
        Surface::Desktop => "VMs",
        Surface::Files => "File Manager",
        _ => surface.label(),
    }
}

/// Group label for a surface, or an empty string for dedicated-only surfaces.
pub(crate) fn launcher_group_label(surface: Surface) -> &'static str {
    if matches!(
        surface,
        Surface::Workbench | Surface::MeshView | Surface::Explorer
    ) {
        return "Mesh Control";
    }
    if matches!(surface, Surface::System | Surface::Storage | Surface::About) {
        return "System";
    }
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map_or("", |group| group.label)
}

#[cfg(test)]
pub(crate) fn launcher_group_accent(surface: Surface) -> Option<egui::Color32> {
    if matches!(
        surface,
        Surface::Workbench | Surface::MeshView | Surface::Explorer
    ) {
        return Some(Style::ACCENT_MESH);
    }
    if matches!(surface, Surface::System | Surface::Storage | Surface::About) {
        return Some(Style::ACCENT_WORKLOADS);
    }
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map(|group| group.accent)
}

/// Rasterize and cache a tinted registry glyph at the requested logical size.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn icon_texture(
    ctx: &egui::Context,
    id: IconId,
    logical_px: f32,
    tint: egui::Color32,
) -> Option<TextureHandle> {
    let size_px = (logical_px * ctx.pixels_per_point()).round().max(1.0) as u32;
    let tint = Style::resolve_color(ctx, tint).to_array();
    let key = egui::Id::new(("surface-icon", id.name(), size_px, tint));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    let texture = icon_image(id, size_px, tint).ok().map(|image| {
        let color = egui::ColorImage::from_rgba_unmultiplied(image.size_usize(), &image.rgba);
        ctx.load_texture(id.name(), color, TextureOptions::LINEAR)
    });
    ctx.data_mut(|data| data.insert_temp(key, texture.clone()));
    texture
}

/// A bounded summary of one open/detected remote session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRailEntry {
    id: Option<String>,
    label: String,
    protocol: &'static str,
    reason: Option<String>,
    retry_guidance: Option<&'static str>,
}

impl SessionRailEntry {
    /// Construct a pending session without a broker id.
    pub fn new(label: impl Into<String>, protocol: &'static str) -> Self {
        Self::with_id(None, label, protocol)
    }

    /// Construct a broker-backed session.
    pub fn with_session_id(
        id: impl Into<String>,
        label: impl Into<String>,
        protocol: &'static str,
    ) -> Self {
        Self::with_id(Some(id.into()), label, protocol)
    }

    fn with_id(id: Option<String>, label: impl Into<String>, protocol: &'static str) -> Self {
        Self {
            id,
            label: truncate_session_label(&label.into()),
            protocol,
            reason: None,
            retry_guidance: None,
        }
    }

    /// Attach a bounded App VM diagnostic and honest next-step guidance.
    ///
    /// This is presentation state only: the guidance deliberately does not
    /// claim that a retry or transport transition has happened.
    pub(crate) fn with_app_status(
        mut self,
        reason: Option<String>,
        retry_guidance: Option<&'static str>,
    ) -> Self {
        self.reason = reason;
        self.retry_guidance = retry_guidance;
        self
    }

    /// Broker session id, when present.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Bounded human label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Short protocol/state badge.
    #[must_use]
    pub const fn protocol(&self) -> &'static str {
        self.protocol
    }

    /// Bounded diagnostic supplied by the App VM lifecycle event.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Honest user-facing next step, when the lifecycle state needs recovery.
    #[must_use]
    pub const fn retry_guidance(&self) -> Option<&'static str> {
        self.retry_guidance
    }

    /// The compact secondary line shown by the control center.
    #[must_use]
    pub fn status_detail(&self) -> Option<String> {
        match (self.reason(), self.retry_guidance()) {
            (Some(reason), Some(guidance)) => Some(format!("{reason} · {guidance}")),
            (Some(reason), None) => Some(reason.to_owned()),
            (None, Some(guidance)) => Some(guidance.to_owned()),
            (None, None) => None,
        }
    }
}

/// Live session snapshot retained for switcher/preview presentation.
#[derive(Clone)]
pub struct SessionPreviewTexture {
    /// Broker session id, when present.
    pub(crate) id: Option<String>,
    /// Bounded session label.
    pub(crate) label: String,
    /// Protocol badge.
    pub(crate) protocol: &'static str,
    /// Decoded guest texture.
    pub(crate) texture: TextureHandle,
}

impl std::fmt::Debug for SessionPreviewTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionPreviewTexture")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("protocol", &self.protocol)
            .field("texture_size", &self.texture.size())
            .finish()
    }
}

impl SessionPreviewTexture {
    /// Construct a bounded preview descriptor.
    pub(crate) fn new(
        id: Option<String>,
        label: impl Into<String>,
        protocol: &'static str,
        texture: TextureHandle,
    ) -> Self {
        Self {
            id,
            label: truncate_session_label(&label.into()),
            protocol,
            texture,
        }
    }
}

/// Compact remote-desktop source summary retained for chooser consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopRailSource {
    /// Stable chooser source id.
    pub(crate) id: String,
    /// Human source label.
    pub(crate) label: String,
    /// Serving node.
    pub(crate) node: String,
    /// Protocol badge.
    pub(crate) protocol: &'static str,
    /// Whether a connect may be requested.
    pub(crate) connectable: bool,
    /// Favorite marker.
    pub(crate) favorite: bool,
    /// Recent marker.
    pub(crate) recent: bool,
}

impl DesktopRailSource {
    /// Construct a bounded source summary.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        node: impl Into<String>,
        protocol: &'static str,
        connectable: bool,
        favorite: bool,
        recent: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: truncate_session_label(&label.into()),
            node: truncate_session_label(&node.into()),
            protocol,
            connectable,
            favorite,
            recent,
        }
    }
}

fn truncate_session_label(label: &str) -> String {
    const MAX_CHARS: usize = 24;
    let mut out: String = label.chars().take(MAX_CHARS).collect();
    if label.chars().count() > MAX_CHARS {
        out.push_str("...");
    }
    out
}

/// Whether a raw interaction activates by pointer or focused Enter/Space.
pub(crate) fn response_activated(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_covers_every_launchable_surface_once() {
        let projected: Vec<_> = LAUNCHER_GROUPS
            .iter()
            .flat_map(|group| group.surfaces.iter().copied())
            .collect();
        assert_eq!(
            projected.len(),
            Surface::ALL.len() - TOOL_TRAY_SURFACES.len() - 1,
            "Files remains a deliberate non-launcher shell route"
        );
        for surface in Surface::ALL {
            assert_eq!(
                projected.iter().filter(|item| **item == surface).count(),
                usize::from(!is_tool_tray_surface(surface) && !matches!(surface, Surface::Files)),
                "{surface:?} has the wrong central-launcher membership"
            );
        }
        assert!(Surface::ALL.contains(&Surface::Workers));
        assert!(!Surface::ALL.contains(&Surface::Phones));
        assert_eq!(Surface::Workers.icon_id(), IconId::Node);
        assert_eq!(
            TOOL_TRAY_SURFACES,
            [
                Surface::Workers,
                Surface::Music,
                Surface::Media,
                Surface::Terminal,
            ]
        );
        assert!(!Surface::ALL.contains(&Surface::System));
        assert!(!Surface::ALL.contains(&Surface::Storage));
        assert!(!Surface::ALL.contains(&Surface::About));

        assert_eq!(Surface::Communications.label(), "Mesh Teams");
        assert_eq!(Surface::Communications.icon_id(), IconId::Teams);
        assert_eq!(
            launcher_group_accent(Surface::Communications),
            Some(Style::ACCENT_TEAMS)
        );
        let web = LAUNCHER_GROUPS
            .iter()
            .find(|group| group.label == "Web")
            .expect("the Browser must own the Web launcher group");
        assert_eq!(web.surfaces, &[Surface::Browser]);
    }

    #[test]
    fn visual_inventory_covers_every_launchable_surface_and_only_allows_governed_boundaries() {
        assert_eq!(SURFACE_VISUAL_INVENTORY.len(), Surface::ALL.len());
        for surface in Surface::ALL {
            assert_eq!(
                SURFACE_VISUAL_INVENTORY
                    .iter()
                    .filter(|item| item.surface == surface)
                    .count(),
                1,
                "{surface:?} needs a complete WL-UX-009 visual classification"
            );
        }

        let exceptions: Vec<_> = SURFACE_VISUAL_INVENTORY
            .iter()
            .filter(|item| item.boundary != VisualBoundary::Construct)
            .map(|item| (item.surface, item.boundary))
            .collect();
        assert_eq!(
            exceptions,
            vec![
                (Surface::Desktop, VisualBoundary::FocusedVdiPixels),
                (Surface::Browser, VisualBoundary::BrowserVmGuest),
                (Surface::MapsLocation, VisualBoundary::MapsContentColour),
            ]
        );
        let browser = SURFACE_VISUAL_INVENTORY
            .iter()
            .find(|item| item.surface == Surface::Browser)
            .expect("Browser must remain in the launchable surface inventory");
        assert_eq!(browser.app_frame, VisualAdoption::Adopted);
        assert_eq!(browser.navigation, VisualAdoption::Partial);
        assert!(SURFACE_VISUAL_INVENTORY
            .iter()
            .all(|item| item.tooltips == VisualAdoption::Adopted));
        assert!(SURFACE_VISUAL_INVENTORY
            .iter()
            .all(|item| item.licensing == VisualAdoption::Gap));
    }

    #[test]
    fn dock_launcher_groups_match_operator_survey() {
        let projected: Vec<_> = DOCK_LAUNCHER_GROUPS
            .iter()
            .map(|group| (group.label, group.surfaces.to_vec()))
            .collect();
        assert_eq!(
            projected,
            vec![
                // Operator survey wording: "VMs and Terminal" — the shipped
                // VM/session launcher is `Surface::Desktop` ("Remote Sessions").
                ("Infra", vec![Surface::Desktop, Surface::Terminal]),
                ("Ops", vec![Surface::MapsLocation, Surface::Communications]),
                (
                    "Life",
                    vec![Surface::Music, Surface::Media, Surface::Browser],
                ),
            ]
        );

        let docked: Vec<_> = DOCK_LAUNCHER_GROUPS
            .iter()
            .flat_map(|group| group.surfaces.iter().copied())
            .collect();
        for surface in [
            Surface::Workers,
            Surface::InfraCode,
            Surface::Files,
            Surface::Phones,
            Surface::ThisNode,
        ] {
            assert!(
                !docked.contains(&surface),
                "{surface:?} stays in the full Springboard/Spotlight catalog, not the compact dock"
            );
        }
        assert_eq!(dock_launcher_group_label(Surface::Terminal), "Infra");
        assert_eq!(dock_launcher_group_label(Surface::Communications), "Ops");
        assert_eq!(dock_launcher_group_label(Surface::Browser), "Life");
        assert_eq!(dock_launcher_group_label(Surface::Workers), "");
        assert_eq!(dock_launcher_surface_label(Surface::Desktop), "VMs");
        assert_eq!(
            dock_launcher_surface_label(Surface::MapsLocation),
            "Maps & Location"
        );
    }

    #[test]
    fn embedded_surface_catalog_excludes_the_shell_host() {
        assert_eq!(EMBEDDED_SURFACE_CRATES.len(), 8);
        assert!(EMBEDDED_SURFACE_CRATES
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(
            EMBEDDED_SURFACE_CRATES.contains(&"mde-bookmarks-egui"),
            "the Browser-owned bookmark manager is rendered by web::web_panel and must remain in the shipped shell catalog"
        );
        assert!(!EMBEDDED_SURFACE_CRATES.contains(&"mde-shell-egui"));
    }

    #[test]
    fn icon_loader_caches_the_same_uploaded_texture() {
        let context = egui::Context::default();
        Style::install(&context);
        let first = icon_texture(&context, IconId::Browser, 24.0, Style::TEXT)
            .expect("Browser registry glyph must rasterize");
        let second = icon_texture(&context, IconId::Browser, 24.0, Style::TEXT)
            .expect("cached Browser registry glyph must remain available");
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn session_labels_remain_bounded_ascii() {
        let entry = SessionRailEntry::new("abcdefghijklmnopqrstuvwxyz", "RDP");
        assert_eq!(entry.label(), "abcdefghijklmnopqrstuvwx...");
    }

    #[test]
    fn app_status_keeps_reason_and_guidance_separate_from_transport_badge() {
        let entry = SessionRailEntry::with_session_id("s1", "Writer", "OFFLINE").with_app_status(
            Some("guest is unavailable".to_owned()),
            Some("Retry from Desktop"),
        );
        assert_eq!(entry.protocol(), "OFFLINE");
        assert_eq!(entry.reason(), Some("guest is unavailable"));
        assert_eq!(entry.retry_guidance(), Some("Retry from Desktop"));
        assert_eq!(
            entry.status_detail().as_deref(),
            Some("guest is unavailable · Retry from Desktop")
        );
    }
}
