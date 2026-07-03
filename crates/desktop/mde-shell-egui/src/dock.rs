//! The shell **dock** — the full-width surface launcher **taskbar** pinned to the
//! bottom edge (NAVBAR-1, superseding the E12-3b left rail).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a flat Carbon taskbar that selects which surface fills the
//! shell body — the mesh-control [`Workbench`](Surface::Workbench) (This Node →
//! Fleet, MV-6), this node's local VM [`Instances`](Surface::Instances) (the
//! cloud-hypervisor broker, E12-7), the brokered VM [`Desktop`](Surface::Desktop)
//! (VDI, egui-native), the embedded app surfaces (Music / Media / Files / Voice /
//! Browser / Terminal / Editor), plus the unified [`Chat`](Surface::Chat) surface
//! — the ONE notification interface (ICQ roster + folded alerts + clipboard clips,
//! NOTIFY-CHAT). One surface shows at a time; the Workbench is always one click away.
//!
//! The bar lays its entries out **horizontally**, partitioned into three
//! declarative [`Group`]s — mesh-control ∣ apps ∣ system — with a thin Carbon
//! divider and the system group pushed hard-right (tray-style order, NAVBAR-2).
//! Each entry is an icon-first glyph cell: the brand glyph over a tiny always-on
//! caption, an accent top-border + brighter tint when active, a hover highlight +
//! `hint` tooltip (NAVBAR-3).
//!
//! The bar is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.

use mde_egui::egui::{self, Align2, FontId, TextureHandle, TextureOptions};
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

/// The three declarative sections a [`Surface`] belongs to on the bottom taskbar
/// (NAVBAR-2). The partition drives the bar's layout: mesh-control and apps pack
/// from the left (split by a Carbon divider), and the system group is pushed
/// hard-right (tray-style order #13). A single [`Surface::group`] classifier keeps
/// the partition declarative — it only *selects*; [`Surface::ALL`] stays the one
/// ordering authority.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    /// Mesh-control surfaces — Workbench, Mesh Map, Instances, Desktop.
    MeshControl,
    /// App surfaces — Music, Media, Files, Voice, Browser, Terminal, Editor, Chat.
    Apps,
    /// System surfaces — System, Storage, About (the tray-side group).
    System,
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

    /// Which taskbar [`Group`] this surface renders in (NAVBAR-2). The locked
    /// partition: mesh-control (Workbench, Mesh Map, Instances, Desktop) ∣ apps
    /// (Music, Media, Files, Voice, Browser, Terminal, Editor, Chat) ∣ system
    /// (System, Storage, About). Exhaustive by construction — every variant maps,
    /// so a new surface must pick a group.
    const fn group(self) -> Group {
        match self {
            Surface::Workbench | Surface::MeshView | Surface::Instances | Surface::Desktop => {
                Group::MeshControl
            }
            Surface::Music
            | Surface::Media
            | Surface::Files
            | Surface::Voice
            | Surface::Browser
            | Surface::Terminal
            | Surface::Editor
            | Surface::Chat => Group::Apps,
            Surface::System | Surface::Storage | Surface::About => Group::System,
        }
    }
}

/// The taskbar height in logical points (design lock #6) — a standard 48px bar,
/// mirroring the chrome strip (`SP_XL + SP_M`). Every value stays on the 8px grid;
/// `main.rs` mounts the bottom panel at exactly this height. (`pub`, not
/// `pub(crate)`, is the `clippy::redundant_pub_crate` form for a crate-visible item
/// in a private module — this is the one dock symbol `main.rs` reads.)
pub const TASKBAR_H: f32 = Style::SP_XL + Style::SP_M;

/// The fixed width of one glyph cell — room for the ~24px glyph plus its tiny
/// always-on caption beneath (design lock #5/#6). Two base units (`SP_XL * 2`).
/// Private: only the bar's own layout + tests read it.
const CELL_W: f32 = Style::SP_XL * 2.0;

/// The glyph edge in logical points — the ~24px brand icon that leads each cell
/// (design lock #6). Rasterized crisp at the physical pixel size by `icon_texture`.
const ICON_LOGICAL: f32 = Style::SP_L;

/// Render the surface launcher as a full-width bottom **taskbar** into `ui`,
/// selecting the active [`Surface`] (NAVBAR-1..3). A click on a cell makes that
/// surface active; the active one reads as selected (accent top-border + solid
/// glyph tint).
///
/// The entries lay out horizontally, partitioned into three [`Group`]s: mesh-control
/// and apps pack from the left (split by a thin Carbon divider), then a flexible
/// gap, then the system group pushed hard-right (tray-style order #13). Each cell is
/// icon-first — the [`brand::icons`](mde_theme::brand::icons) glyph over a tiny
/// always-on caption — with a hover highlight + `hint` tooltip.
pub(crate) fn taskbar(ui: &mut egui::Ui, active: &mut Surface) {
    // NAVBAR-1 — a hairline top divider on the seam between the surface body above
    // and the bar, drawn from the installed BORDER stroke (a Style token, not a raw
    // colour/width — §4). The flat SURFACE fill is the panel frame (`main.rs`).
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(ui.max_rect().x_range(), ui.max_rect().top(), hairline);

    ui.horizontal(|ui| {
        // A seamless, edge-to-edge bar (design lock #8): no inter-cell gaps — each
        // cell carries its own internal padding around the centred glyph.
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

        // NAVBAR-2 — mesh-control, a thin divider, then apps (both left-packed).
        render_group(ui, Group::MeshControl, active);
        divider(ui);
        render_group(ui, Group::Apps, active);

        // NAVBAR-2 — flexible space, then the system group hard-right (tray-style
        // order #13): a right-to-left sub-layout consumes the remaining width and
        // packs System · Storage · About against the right edge (About furthest
        // right), leaving room on the right for the status-tray fold-in (NAVBAR-4).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            for surface in Surface::ALL
                .iter()
                .rev()
                .copied()
                .filter(|s| s.group() == Group::System)
            {
                cell(ui, surface, active);
            }
        });
    });
}

/// Render one [`Group`]'s surfaces as cells, in the canonical [`Surface::ALL`] nav
/// order (the single ordering authority — the partition only *selects*, never
/// re-orders).
fn render_group(ui: &mut egui::Ui, which: Group, active: &mut Surface) {
    for surface in Surface::ALL.iter().copied().filter(|s| s.group() == which) {
        cell(ui, surface, active);
    }
}

/// A thin vertical Carbon divider between two groups — a short, vertically-inset
/// hairline in the BORDER token (§4), the horizontal analogue of the old rail's
/// separators.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(Style::SP_S, ui.available_height()),
        egui::Sense::hover(),
    );
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().vline(
        rect.center().x,
        (rect.top() + Style::SP_S)..=(rect.bottom() - Style::SP_S),
        stroke,
    );
}

/// One taskbar entry — an icon-first glyph cell (NAVBAR-3): the brand glyph over a
/// tiny always-on caption, an accent top-border + brighter tint when active, a
/// hover highlight + `hint` tooltip, and a click that selects the surface.
fn cell(ui: &mut egui::Ui, surface: Surface, active: &mut Surface) {
    let selected = *active == surface;
    let cell_h = ui.available_height();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CELL_W, cell_h), egui::Sense::click());
    // Hover reveals the surface's one-line hint (design lock #10).
    let response = response.on_hover_text(surface.hint());
    let hovered = response.hovered();

    // A painter clone so `egui::Image::paint_at` can still borrow `ui` (splash.rs).
    let painter = ui.painter().clone();

    // Cell background: the selected cell wears the accent selection wash, a hovered
    // one the raised SURFACE_HI — both Style tokens (§4, design lock #10).
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }

    // Active mark (design lock #7): an accent strip along the cell's top edge.
    if selected {
        let strip =
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), Style::SP_XS));
        painter.rect_filled(strip, egui::CornerRadius::ZERO, Style::ACCENT);
    }

    // Two-tone tint (design lock #9): the active glyph reads solid in the brand
    // ACCENT, a hovered one brightens to full TEXT, the rest sit dim at TEXT_DIM.
    // The brand SVG set is a single `currentColor` variant (no separate outline
    // artwork), so "filled vs outline" is approximated by tint intensity — every
    // value a Style token, never a raw colour (§4).
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

    // The glyph + caption vertical stack, centred in the cell. A glyph load failure
    // fails soft to the caption alone (§7).
    let content_h = ICON_LOGICAL + Style::SP_XS + Style::SMALL;
    let glyph_top = rect.top() + (cell_h - content_h) / 2.0;
    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), ICON_LOGICAL, tint) {
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - ICON_LOGICAL / 2.0, glyph_top),
            egui::vec2(ICON_LOGICAL, ICON_LOGICAL),
        );
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    // The tiny always-on caption (design lock #5), centred beneath the glyph.
    painter.text(
        egui::pos2(
            rect.center().x,
            glyph_top + ICON_LOGICAL + Style::SP_XS + Style::SMALL / 2.0,
        ),
        Align2::CENTER_CENTER,
        surface.label(),
        FontId::proportional(Style::SMALL),
        tint,
    );

    if response.clicked() {
        *active = surface;
    }
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
    use super::{icon_texture, taskbar, Group, Surface, CELL_W, ICON_LOGICAL, TASKBAR_H};
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

    // --- NAVBAR-2: the three-group partition --------------------------------------

    #[test]
    fn the_three_groups_partition_all_fifteen_surfaces() {
        // The locked partition (NAVBAR-2): mesh-control (4) ∣ apps (8) ∣ system (3).
        let mesh = [
            Surface::Workbench,
            Surface::MeshView,
            Surface::Instances,
            Surface::Desktop,
        ];
        let apps = [
            Surface::Music,
            Surface::Media,
            Surface::Files,
            Surface::Voice,
            Surface::Browser,
            Surface::Terminal,
            Surface::Editor,
            Surface::Chat,
        ];
        let system = [Surface::System, Surface::Storage, Surface::About];
        for s in mesh {
            assert_eq!(s.group(), Group::MeshControl, "{s:?} not mesh-control");
        }
        for s in apps {
            assert_eq!(s.group(), Group::Apps, "{s:?} not apps");
        }
        for s in system {
            assert_eq!(s.group(), Group::System, "{s:?} not system");
        }

        // Every surface in ALL lands in exactly one group; the three filters cover
        // all fifteen — a true partition, no surface dropped or double-counted.
        let mesh_n = Surface::ALL
            .iter()
            .filter(|s| s.group() == Group::MeshControl)
            .count();
        let apps_n = Surface::ALL
            .iter()
            .filter(|s| s.group() == Group::Apps)
            .count();
        let sys_n = Surface::ALL
            .iter()
            .filter(|s| s.group() == Group::System)
            .count();
        assert_eq!((mesh_n, apps_n, sys_n), (4, 8, 3), "group cardinalities");
        assert_eq!(
            mesh_n + apps_n + sys_n,
            Surface::ALL.len(),
            "the three groups must partition ALL"
        );
    }

    // --- NAVBAR-1/3: the bar mounts, renders, and switches surface on a click -----

    #[test]
    fn the_taskbar_renders_and_caches_the_glyphs_headless() {
        // Drive one headless frame of the full-width bottom taskbar (the same
        // Context::run → tessellate path the DRM runner uses, minus the GPU): it
        // must draw primitives without panicking, and every surface glyph must
        // resolve to a real texture through the memory-cached loader.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 640.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("shell-taskbar")
                .exact_height(TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| taskbar(ui, &mut active));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the taskbar drew nothing");

        for surface in Surface::ALL {
            assert!(
                icon_texture(&ctx, surface.icon_id(), ICON_LOGICAL, Style::TEXT_DIM).is_some(),
                "{surface:?} glyph failed to rasterize + upload"
            );
        }
    }

    #[test]
    fn clicking_a_taskbar_cell_selects_that_surface() {
        // NAVBAR-3 preserves the click→select behaviour. Mount the real bottom bar
        // and click the leftmost cell (Workbench, the mesh-control head). egui hit-
        // tests a press against the settled widget rects, so prime a couple of
        // no-event frames first, then press one frame + release the next (the egui
        // click model), and the active surface follows the click.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::About;
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 600.0));
        // The first cell is flush-left (edge-to-edge, lock #8); its centre is one
        // half-cell in from the left, at the bar's mid-height — derived from the
        // layout constants, not a magic number.
        let click = egui::pos2(CELL_W / 2.0, 600.0 - TASKBAR_H / 2.0);
        let run = |active: &mut Surface, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            // The frame output is unused here — the test asserts on `active`, not
            // the tessellation (that is the headless-render test's job).
            let _ = ctx.run(input, |ctx| {
                egui::TopBottomPanel::bottom("shell-taskbar")
                    .exact_height(TASKBAR_H)
                    .frame(egui::Frame::default().fill(Style::SURFACE))
                    .show(ctx, |ui| taskbar(ui, active));
            });
        };
        let press = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        // Prime: settle the layout so egui has the cell rects registered.
        run(&mut active, Vec::new());
        run(&mut active, Vec::new());
        // Move onto the Workbench cell + press, then release the next frame.
        run(&mut active, vec![egui::Event::PointerMoved(click), press]);
        run(&mut active, vec![release]);
        assert_eq!(
            active,
            Surface::Workbench,
            "clicking the Workbench cell selected it"
        );
    }
}
