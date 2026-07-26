//! Canonical Construct surface taxonomy and shared surface presentation helpers.
//!
//! This module is deliberately chrome-neutral: Springboard, Spotlight, the app
//! switcher, Car, and tests all consume the same surface order, grouping, labels,
//! Carbon glyph loader, and session-summary types.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};

/// Which surface fills the shell body.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Surface {
    /// Unified fleet, mesh-map, workbench, and discovered-unit interface.
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
    /// Bookmarks manager.
    Bookmarks,
    /// Maps, location, and vehicle management.
    MapsLocation,
    /// Local and remote terminal.
    Terminal,
    /// Paired-phone management.
    Phones,
    /// Collaboration and communications hub.
    Communications,
    /// Unified node-local settings, storage, and hardware/identity interface.
    ThisNode,
    /// Host settings and controls.
    System,
    /// Disk and partition management.
    Storage,
    /// Platform identity and legal information.
    About,
    /// Timers and alarms, reached from the status clock.
    Timers,
    /// Car-profile dashboard home.
    AutoHome,
}

#[allow(clippy::use_self)]
impl Surface {
    /// Every Springboard/Spotlight surface in canonical keyboard order.
    pub(crate) const ALL: [Surface; 13] = [
        Surface::FleetMesh,
        Surface::InfraCode,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Browser,
        Surface::Bookmarks,
        Surface::MapsLocation,
        Surface::Terminal,
        Surface::Phones,
        Surface::ThisNode,
        Surface::Communications,
    ];

    /// The shared Carbon glyph for this surface.
    pub(crate) const fn icon_id(self) -> IconId {
        match self {
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
            Surface::Bookmarks => IconId::Bookmarks,
            Surface::MapsLocation => IconId::MapsLocation,
            Surface::Terminal => IconId::Terminal,
            Surface::Phones => IconId::Phones,
            Surface::Communications => IconId::Teams,
            Surface::ThisNode | Surface::System => IconId::Settings,
            Surface::Storage => IconId::Storage,
            Surface::About | Surface::Timers => IconId::Mark,
        }
    }

    /// Human-facing label shared by every launcher and switcher.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::FleetMesh | Surface::Workbench | Surface::MeshView | Surface::Explorer => {
                "Fleet & Mesh"
            }
            Surface::InfraCode => "Infra as Code",
            Surface::Desktop => "Remote Sessions",
            Surface::Music => "Music",
            Surface::Media => "Media",
            Surface::Files => "Files",
            Surface::Browser => "Browser",
            Surface::Bookmarks => "Bookmarks",
            Surface::MapsLocation => "Maps & Location",
            Surface::Terminal => "Terminal",
            Surface::Phones => "Phones",
            Surface::Communications => "Mesh Teams",
            Surface::ThisNode | Surface::System | Surface::Storage | Surface::About => "This Node",
            Surface::Timers => "Timers & Alarms",
            Surface::AutoHome => "Car Home",
        }
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

/// The eight persistent launcher taxonomy groups. Every [`Surface::ALL`] entry
/// appears exactly once; the single Springboard desktop uses the flattened
/// taxonomy order while Spotlight and the switcher retain these groups.
pub(crate) const LAUNCHER_GROUPS: [LauncherGroup; 8] = [
    LauncherGroup {
        label: "Mesh Control",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::FleetMesh, Surface::InfraCode],
    },
    LauncherGroup {
        label: "Desktop & Session",
        accent: Style::ACCENT,
        surfaces: &[Surface::Desktop, Surface::MapsLocation],
    },
    LauncherGroup {
        label: "Media",
        accent: Style::ACCENT_MEDIA,
        surfaces: &[Surface::Music, Surface::Media],
    },
    LauncherGroup {
        label: "Files & Data",
        accent: Style::ACCENT_SYSTEM,
        surfaces: &[Surface::Files],
    },
    LauncherGroup {
        label: "Web",
        accent: Style::ACCENT_WEB,
        surfaces: &[Surface::Browser, Surface::Bookmarks],
    },
    LauncherGroup {
        label: "Developer Tools",
        accent: Style::ACCENT_TERMINALS,
        surfaces: &[Surface::Terminal],
    },
    LauncherGroup {
        label: "Mesh Teams",
        accent: Style::ACCENT_TEAMS,
        surfaces: &[Surface::Phones, Surface::Communications],
    },
    LauncherGroup {
        label: "System",
        accent: Style::ACCENT_WORKLOADS,
        surfaces: &[Surface::ThisNode],
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
        surfaces: &[
            Surface::MapsLocation,
            Surface::Communications,
            Surface::Files,
        ],
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
        assert!(
            count == 1,
            "every Surface::ALL entry must appear in LAUNCHER_GROUPS exactly once",
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

/// Rasterize and cache a tinted Carbon glyph at the requested logical size.
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
        }
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
        assert_eq!(projected.len(), Surface::ALL.len());
        for surface in Surface::ALL {
            assert_eq!(projected.iter().filter(|item| **item == surface).count(), 1);
        }
        assert!(Surface::ALL.contains(&Surface::ThisNode));
        assert!(!Surface::ALL.contains(&Surface::System));
        assert!(!Surface::ALL.contains(&Surface::Storage));
        assert!(!Surface::ALL.contains(&Surface::About));

        assert_eq!(Surface::Communications.label(), "Mesh Teams");
        assert_eq!(Surface::Communications.icon_id(), IconId::Teams);
        assert_eq!(
            launcher_group_accent(Surface::Communications),
            Some(Style::ACCENT_TEAMS)
        );
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
                (
                    "Ops",
                    vec![
                        Surface::MapsLocation,
                        Surface::Communications,
                        Surface::Files,
                    ],
                ),
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
            Surface::FleetMesh,
            Surface::InfraCode,
            Surface::Bookmarks,
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
        assert_eq!(dock_launcher_group_label(Surface::FleetMesh), "");
        assert_eq!(dock_launcher_surface_label(Surface::Desktop), "VMs");
        assert_eq!(dock_launcher_surface_label(Surface::Files), "File Manager");
        assert_eq!(
            dock_launcher_surface_label(Surface::MapsLocation),
            "Maps & Location"
        );
    }

    #[test]
    fn embedded_surface_catalog_excludes_host_and_standalone_panel() {
        assert_eq!(EMBEDDED_SURFACE_CRATES.len(), 8);
        assert!(EMBEDDED_SURFACE_CRATES
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(!EMBEDDED_SURFACE_CRATES.contains(&"mde-shell-egui"));
        assert!(!EMBEDDED_SURFACE_CRATES.contains(&"mde-panel-egui"));
    }

    #[test]
    fn icon_loader_caches_the_same_uploaded_texture() {
        let context = egui::Context::default();
        Style::install(&context);
        let first = icon_texture(&context, IconId::Browser, 24.0, Style::TEXT)
            .expect("Browser Carbon glyph must rasterize");
        let second = icon_texture(&context, IconId::Browser, 24.0, Style::TEXT)
            .expect("cached Browser Carbon glyph must remain available");
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn session_labels_remain_bounded_ascii() {
        let entry = SessionRailEntry::new("abcdefghijklmnopqrstuvwxyz", "RDP");
        assert_eq!(entry.label(), "abcdefghijklmnopqrstuvwx...");
    }
}
