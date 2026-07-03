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

use mde_egui::egui::{self, Align2, FontId, RichText, TextureHandle, TextureOptions};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};

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
    /// The embedded Editor surface (`mde-editor-egui`) — the native Zed-style code
    /// editor (EDITOR epic). EDITOR-1 mounts the scaffold: the editor chrome + the
    /// honest "No file open" empty state (§7); the rope buffer + text widget +
    /// tree-sitter highlighting + tabs/splits land in EDITOR-2 onward.
    Editor,
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
    /// The About surface — the canonical "about this platform" screen (QBRAND-6,
    /// placement lock #13): the official `MDE-QUAZAR-MAIN.png` lockup, the product
    /// name + tagline, the full build identity (version · git hash · date · channel),
    /// and the shipped legal docs + source URL. A pure renderer of the
    /// [`mde_theme::brand`] constants (`crate::about`).
    About,
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
    /// notification interface), then this seat's host-controls System + Storage
    /// surfaces, and finally the About surface (the platform-identity screen).
    pub(crate) const ALL: [Surface; 15] = [
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
        Surface::Editor,
        Surface::Chat,
        Surface::System,
        Surface::Storage,
        Surface::About,
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
            Surface::Editor => "Editor",
            Surface::Chat => "Chat",
            Surface::System => "System",
            Surface::Storage => "Storage",
            Surface::About => "About",
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
            Surface::Editor => {
                "A native, keyboard-driven code editor — open a file to start editing."
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
            Surface::About => {
                "About MDE Quazar — the product lockup, the full build identity, and the shipped licenses."
            }
        }
    }

    /// The [`brand::icons`](mde_theme::brand::icons) glyph this surface draws in
    /// the rail (QBRAND-7). A 1:1 map by name onto the Quasar brand set — every
    /// dock surface has a dedicated line-art glyph and `MeshView` folds onto the
    /// topology-map glyph. The dock never re-draws a glyph; it tints this one
    /// through the shared loader (§6).
    pub(crate) const fn icon_id(self) -> IconId {
        match self {
            Surface::Workbench => IconId::Workbench,
            Surface::MeshView => IconId::MeshView,
            Surface::Instances => IconId::Instances,
            Surface::Desktop => IconId::Desktop,
            Surface::Music => IconId::Music,
            Surface::Media => IconId::Media,
            Surface::Files => IconId::Files,
            Surface::Voice => IconId::Voice,
            Surface::Browser => IconId::Browser,
            Surface::Terminal => IconId::Terminal,
            Surface::Editor => IconId::Editor,
            Surface::Chat => IconId::Chat,
            Surface::System => IconId::System,
            Surface::Storage => IconId::Storage,
            // The About surface wears the product **mark** — the mesh-node
            // constellation glyph that IS the platform identity — fitting the
            // "about this platform" screen and distinct from every surface glyph.
            Surface::About => IconId::Mark,
        }
    }
}

/// The height of a dock entry — unchanged from the pre-glyph `SelectableLabel`
/// row (`add_sized([_, SP_L], …)`), so the rail keeps its compact rhythm.
const ROW_H: f32 = Style::SP_L;
/// The glyph edge in logical points — a 16px brand icon inset in the 24px row.
const ICON_LOGICAL: f32 = Style::SP_M;

/// Render the dock rail into `ui`, selecting the active [`Surface`]. A click on a
/// launcher makes that surface active; the active one reads as selected.
///
/// Each entry leads with its [`brand::icons`](mde_theme::brand::icons) glyph
/// (QBRAND-7), tinted by the active/hover/rest state, followed by the unchanged
/// text label + hover hint.
pub(crate) fn rail(ui: &mut egui::Ui, active: &mut Surface) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("SURFACES")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    for surface in Surface::ALL {
        let response = surface_row(ui, surface, *active == surface).on_hover_text(surface.hint());
        if response.clicked() {
            *active = surface;
        }
        ui.add_space(Style::SP_XS);
    }
}

/// One dock entry — the leading brand glyph plus the text label, laid out as a
/// full-width selectable row. Mirrors the retired `SelectableLabel`: the active
/// row wears the accent selection fill, a hovered one the raised surface, and the
/// label copy + text style are unchanged — only the leading glyph is new.
fn surface_row(ui: &mut egui::Ui, surface: Surface, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::click());

    // The glyph tint follows the active/hover/rest hierarchy the SelectableLabel
    // drew through the installed Visuals: the selected surface reads in the brand
    // ACCENT, a hovered one brightens to full TEXT, the rest sit at TEXT_DIM —
    // every value a shared Style token, never a raw colour (§4). The label copy
    // and its TEXT colour are unchanged, so only the icon restyles.
    let tint = if selected {
        Style::ACCENT
    } else if response.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

    // A painter clone so `egui::Image::paint_at` can still borrow `ui` (splash.rs).
    let painter = ui.painter().clone();

    // Selection / hover background — the SelectableLabel look, read straight from
    // the installed Visuals: the accent selection fill when active, the raised
    // surface on hover.
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(
            rect,
            Style::RADIUS,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }

    // The brand glyph, rasterized crisp at the physical pixel size and tinted, sets
    // where the label starts. A load failure keeps the text label alone (§7): the
    // label falls back to the bare left inset and always draws.
    let text_left = icon_texture(ui.ctx(), surface.icon_id(), ICON_LOGICAL, tint).map_or_else(
        || rect.left() + Style::SP_S,
        |tex| {
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + Style::SP_S,
                    rect.center().y - ICON_LOGICAL / 2.0,
                ),
                egui::vec2(ICON_LOGICAL, ICON_LOGICAL),
            );
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
                .paint_at(ui, icon_rect);
            icon_rect.right() + Style::SP_S
        },
    );

    // The label — the same copy + Style token colour the SelectableLabel drew; the
    // hover hint is added by the caller (unchanged).
    painter.text(
        egui::pos2(text_left, rect.center().y),
        Align2::LEFT_CENTER,
        surface.label(),
        FontId::proportional(Style::BODY),
        Style::TEXT,
    );

    response
}

/// Rasterize + upload a brand glyph, cached in egui memory so a given
/// `(glyph, physical-size, tint)` triple is rasterized through `resvg` **once**
/// and then shared as a cheap ref-counted [`TextureHandle`] — never re-rasterized
/// per frame (the backdrop.rs lock-7 pattern). A failed rasterize caches `None`,
/// so a broken asset fails soft to the bare text label (§7) without retrying
/// every frame.
///
/// The glyph is rasterized at the physical pixel size (`logical × ppp`) and drawn
/// back at the logical size, so it stays DPI-crisp at any `HiDPI` scale — the
/// loader honours the exact requested px.
#[allow(
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // size_px ≥ 1.0 by the .max(1.0) clamp
)]
fn icon_texture(
    ctx: &egui::Context,
    id: IconId,
    logical_px: f32,
    tint: egui::Color32,
) -> Option<TextureHandle> {
    let size_px = (logical_px * ctx.pixels_per_point()).round().max(1.0) as u32;
    let tint = tint.to_array();
    let key = egui::Id::new(("qbrand7-dock-icon", id.name(), size_px, tint));

    // Fast path: the resolved texture (or a cached `None` from an earlier failed
    // decode) is already in egui memory — a cheap ref-counted clone.
    if let Some(cached) = ctx.data_mut(|d| d.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    // Slow path (first paint of this variant): rasterize + upload OUTSIDE the
    // `data_mut` lock. `load_texture` read-locks the context that `data_mut`
    // write-locks, so uploading inside would deadlock the frame (backdrop.rs) —
    // resolve first, then cache the handle.
    let texture = icon_image(id, size_px, tint).ok().map(|img| {
        let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
        ctx.load_texture(id.name(), color, TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    texture
}

#[cfg(test)]
mod tests {
    use super::{icon_texture, rail, Surface, ICON_LOGICAL};
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_theme::brand::icons::{icon_image, IconId};

    #[test]
    fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
        // Fifteen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
        // two VM surfaces (Instances / Desktop), the app surfaces (Music / Media — the
        // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
        // browser, BOOKMARKS-6 / Terminal — the Terminator-class terminal over a real
        // PTY, TERM-16 / Editor — the native Zed-style code editor, EDITOR-1), the
        // unified Chat surface (the ONE notification interface — the standalone
        // Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the
        // host-controls System surface, the Storage surface (GParted-authentic disk
        // mgmt, E12-21), and the About surface (the platform-identity screen, QBRAND-6).
        assert_eq!(Surface::ALL.len(), 15);
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
            Surface::Editor,
            Surface::Chat,
            Surface::System,
            Surface::Storage,
            Surface::About,
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

    // --- QBRAND-7: every dock surface renders a brand::icons glyph ----------------

    #[test]
    fn every_surface_maps_to_a_named_brand_glyph() {
        // The map is 1:1 by name (Workbench→Workbench … MeshView→MeshView), and no
        // surface folds onto the blank text wordmark.
        let cases = [
            (Surface::Workbench, IconId::Workbench),
            (Surface::MeshView, IconId::MeshView),
            (Surface::Instances, IconId::Instances),
            (Surface::Desktop, IconId::Desktop),
            (Surface::Music, IconId::Music),
            (Surface::Media, IconId::Media),
            (Surface::Files, IconId::Files),
            (Surface::Voice, IconId::Voice),
            (Surface::Browser, IconId::Browser),
            (Surface::Terminal, IconId::Terminal),
            (Surface::Editor, IconId::Editor),
            (Surface::Chat, IconId::Chat),
            (Surface::System, IconId::System),
            (Surface::Storage, IconId::Storage),
            (Surface::About, IconId::Mark),
        ];
        assert_eq!(cases.len(), Surface::ALL.len(), "a surface is unmapped");
        for (surface, id) in cases {
            assert_eq!(surface.icon_id(), id, "{surface:?} → wrong glyph");
            assert_ne!(
                id,
                IconId::Wordmark,
                "{surface:?} maps to the blank wordmark"
            );
        }
        // The map is injective — 15 surfaces, 15 distinct glyph names.
        let mut names: Vec<&str> = Surface::ALL.iter().map(|s| s.icon_id().name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Surface::ALL.len(), "surface→glyph map not 1:1");
    }

    #[test]
    fn every_surface_glyph_rasterizes_nonempty() {
        // Each surface's glyph resolves to real ink through the shared loader,
        // tinted by a Style token (no raw hex) — so the rail never draws an empty
        // square.
        let tint = Style::TEXT_DIM.to_array();
        for surface in Surface::ALL {
            let img = icon_image(surface.icon_id(), 32, tint).expect("surface glyph rasterizes");
            let inked = img.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
            assert!(inked > 0, "{surface:?} glyph rasterized empty");
        }
    }

    #[test]
    fn the_rail_renders_and_caches_the_glyphs_headless() {
        // Drive one headless frame of the rail (the same Context::run → tessellate
        // path the DRM runner uses, minus the GPU): it must draw primitives without
        // panicking, and every surface glyph must resolve to a real texture through
        // the memory-cached loader.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(160.0, 640.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| rail(ui, &mut active));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the dock rail drew nothing");

        for surface in Surface::ALL {
            assert!(
                icon_texture(&ctx, surface.icon_id(), ICON_LOGICAL, Style::TEXT_DIM).is_some(),
                "{surface:?} glyph failed to rasterize + upload"
            );
        }
    }
}
