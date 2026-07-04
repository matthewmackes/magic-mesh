//! The shell **dock** — the full-width surface launcher **taskbar** pinned to
//! the bottom edge: the shell's ONE bar (NAVBAR-W10-2, superseding NAVBAR-1's
//! labelled/grouped bar and the E12-3b left rail before it).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a Win10-style taskbar (lock W3 — 24px app glyphs on a bar
//! given top breathing room) that selects which surface fills the shell body — the mesh-control
//! [`Workbench`](Surface::Workbench), the live Mesh Map, the VM surfaces
//! (Instances / Desktop), the embedded app surfaces (Music / Media / Files /
//! Voice / Browser / Terminal / Editor), the unified [`Chat`](Surface::Chat)
//! surface, and the System / Storage / About screens. One surface shows at a
//! time; the Workbench is always one click away.
//!
//! The app row leads with the **Workbench** as a standalone anchor, then the
//! surfaces gathered into six labelled **groups** (PICKER-1: Comms · Workloads ·
//! Terminals · Mesh · System · Media) — each group preceded by a rotated
//! bottom-to-top accent label + a Carbon-blue hairline to its left, its 24px
//! brand glyph cells kept in [`Surface::ALL`] order. The active cell still wears
//! a **bottom-edge accent underline** + the subtle selection wash; hover is a
//! fill only — no per-icon captions, no tooltips anywhere. After a flexible gap
//! the right corner cluster: the **Settings** (host-controls) gear button just
//! left of the right-justified status **tray** + clock (`tray.rs`), and — pinned
//! to the very bottom-right corner PAST the tray — the Win10 **Show Desktop**
//! sliver: a thin icon-only button that routes to
//! [`Surface::Desktop`](Surface::Desktop).
//!
//! The bar is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::Style;
use mde_theme::brand::icons::{icon_image, IconId};

use crate::tray::{self, TrayInputs, TrayState};

/// Which surface fills the shell body.
///
/// [`Workbench`](Self::Workbench) is the default: the shell opens on the
/// mesh-control Workbench — the other surfaces are the panels beside it.
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for
/// crate-visible items in a private module — like `TASKBAR_H` below.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Surface {
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
    /// (E12-15). Owns ALL host-control interaction (lock 3); the taskbar tray
    /// keeps only read-only status icons.
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
// purpose: the explicit type keeps the `ALL` table and the glyph map scannable
// side by side (a launcher reads clearer than a wall of `Self::`). Opt the block
// out of `clippy::use_self` rather than thread `Self::` through every arm.
#[allow(clippy::use_self)]
impl Surface {
    /// Every surface in canonical order — the ordering authority the picker is
    /// built + checked against: the Workbench (mesh-control home) first, then the
    /// live Mesh Map, the local VM Instances broker + the brokered Desktop, the
    /// app surfaces, the unified Chat surface (the ONE notification interface),
    /// and the System / Storage / About screens. PICKER-1 gathers these into the
    /// labelled [`GROUPS`] (the Workbench leads standalone), preserving this
    /// relative order within each group (L7); a compile-time guard keeps the two
    /// tables in sync.
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

    /// The [`brand::icons`](mde_theme::brand::icons) glyph this surface draws in
    /// the bar (QBRAND-7). A 1:1 map by name onto the Quasar brand set — every
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
            // The System (host-controls) surface is the dock's right-side Settings
            // button (PICKER-2) — it wears the toothed **cog** glyph, the Win10
            // settings-gear idiom, distinct from the spoked legacy System glyph.
            Surface::System => IconId::Settings,
            Surface::Storage => IconId::Storage,
            // The About surface wears the product **mark** — the mesh-node
            // constellation glyph that IS the platform identity — fitting the
            // "about this platform" screen and distinct from every surface glyph.
            Surface::About => IconId::Mark,
        }
    }
}

/// The taskbar height in logical points — a Win10-style bar given extra breathing
/// room above the icon row (`SP_XL + SP_M + SP_S` on the 8px grid = the former
/// 40px bar plus an `SP_M` [`TASKBAR_TOP_PAD`] top strip). `main.rs` mounts the
/// bottom panel at exactly this height. (`pub`, not `pub(crate)`, is the
/// `clippy::redundant_pub_crate` form for a crate-visible item in a private
/// module.)
pub const TASKBAR_H: f32 = Style::SP_XL + Style::SP_M + Style::SP_S;

/// The empty breathing room ABOVE the icon row (`SP_M`) — the taller bar is
/// bottom-biased, so the 24px glyphs sit low (Win10-taskbar feel) with this much
/// clear space over them. The icon band is the bottom `TASKBAR_H − TASKBAR_TOP_PAD`
/// of each cell; the active underline still hugs the very bottom edge.
const TASKBAR_TOP_PAD: f32 = Style::SP_M;

/// The fixed width of one icon-only glyph cell (lock W4 — no caption, so the
/// cell shrinks to suit the 24px glyph): `SP_XL + SP_M` on the 8px grid.
/// Private: only the bar's own layout + tests read it.
const CELL_W: f32 = Style::SP_XL + Style::SP_M;

/// The app glyph edge in logical points — the Win10 24px taskbar icon (lock
/// W3, `SP_L`). Rasterized crisp at the physical pixel size by `icon_texture`.
const ICON_LOGICAL: f32 = Style::SP_L;

/// The width of the Win10 **"Show Desktop"** sliver pinned to the bar's far-right
/// corner (past the tray) — a thin button, deliberately narrower than a normal
/// [`CELL_W`] cell (`SP_L + SP_S` on the 8px grid), yet wide enough to centre the
/// 24px Desktop glyph with a hair of breathing room.
const SHOW_DESKTOP_W: f32 = Style::SP_L + Style::SP_S;

/// The active cell's **bottom-edge accent underline** (lock W5 — the Win10
/// running/active idiom, replacing the old top strip): a full-width strip,
/// `SP_XS` tall, hugging the cell's bottom edge. Pure geometry, unit-tested.
fn underline(cell: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(cell.left(), cell.bottom() - Style::SP_XS),
        egui::vec2(cell.width(), Style::SP_XS),
    )
}

// ── PICKER-1: the app row grouped into named sections ───────────────────────

/// A named section of the app row (PICKER-1): a rotated bottom-to-top accent
/// label + a Carbon-blue hairline, drawn to the LEFT of the group's icon cells
/// (the existing 24px cells, unchanged). The Workbench is NOT in any group — it
/// leads the row as a standalone anchor.
struct Group {
    /// The section heading, painted rotated (bottom-to-top) in [`Self::accent`].
    label: &'static str,
    /// The group's identity colour — the label tint (design L4).
    accent: egui::Color32,
    /// The group's surfaces, kept in [`Surface::ALL`] relative order (lock L7).
    surfaces: &'static [Surface],
}

// PICKER-2: the per-group accent colours are the shared categorical tokens on
// `Style` (`ACCENT_COMMS`..`ACCENT_MEDIA`) — the SAME six hues EXPLORER-15
// consumes for the unit explorer's per-category identity (design O8). One colour
// language across the picker + the explorer, defined ONCE in the token module
// (`mde_egui::Style`) and consumed by both; the raw hex lives only there (§4, no
// raw colours here). The Carbon-blue hairline reuses the interactive-blue token
// [`Style::ACCENT`].

/// The six labelled groups in their locked left-to-right order (L5), each
/// listing its surfaces in [`Surface::ALL`] relative order (L7). THREE surfaces
/// sit outside every group: the **Workbench** leads the row as the standalone
/// anchor, **System** is the right-side Settings button (rendered just left of the
/// tray), and **Desktop** is the far-right Win10 [`show_desktop_sliver`] past the
/// tray; every other surface appears here exactly once (About lives in System's
/// group) — the union with those three reproduces all 15 of [`Surface::ALL`].
/// Drives the app-row render + the shell tests (the one grouping authority).
const GROUPS: [Group; 6] = [
    Group {
        label: "Comms",
        accent: Style::ACCENT_COMMS,
        surfaces: &[Surface::Voice, Surface::Chat],
    },
    Group {
        label: "Workloads",
        accent: Style::ACCENT_WORKLOADS,
        surfaces: &[Surface::Instances],
    },
    Group {
        label: "Terminals",
        accent: Style::ACCENT_TERMINALS,
        surfaces: &[Surface::Browser, Surface::Terminal, Surface::Editor],
    },
    Group {
        label: "Mesh",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::MeshView],
    },
    Group {
        label: "System",
        accent: Style::ACCENT_SYSTEM,
        // The System *surface* is the right-side Settings button, not a member
        // here; this group gathers the remaining system-adjacent surfaces.
        surfaces: &[Surface::Files, Surface::Storage, Surface::About],
    },
    Group {
        label: "Media",
        accent: Style::ACCENT_MEDIA,
        surfaces: &[Surface::Music, Surface::Media],
    },
];

// Compile-time guard: the Workbench lead + the right-side System Settings button +
// the far-right Desktop sliver + the six `GROUPS` place every `Surface::ALL` entry
// EXACTLY once — so the picker can never silently drop or duplicate a surface when
// either table changes (add a surface to `ALL` but forget to group it, or list it
// twice, and the crate fails to compile). Keeps `Surface::ALL` the authority the
// render is checked against. Fieldless enums cast to their discriminant in const,
// so this compares by identity.
const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        // Three surfaces are placed outside every group: Workbench (standalone
        // lead), System (right-side Settings button), Desktop (far-right
        // Show-Desktop sliver).
        let mut count = if Surface::Workbench as usize == target
            || Surface::System as usize == target
            || Surface::Desktop as usize == target
        {
            1
        } else {
            0
        };
        let mut g = 0;
        while g < GROUPS.len() {
            let surfaces = GROUPS[g].surfaces;
            let mut s = 0;
            while s < surfaces.len() {
                if surfaces[s] as usize == target {
                    count += 1;
                }
                s += 1;
            }
            g += 1;
        }
        assert!(
            count == 1,
            "every Surface::ALL entry must be placed exactly once across the Workbench lead + the System Settings button + the Desktop sliver + GROUPS",
        );
        i += 1;
    }
};

/// The Carbon-blue group hairline width in logical points — a 1px rule (L3).
const HAIRLINE_W: f32 = 1.0;

/// The group-label point-size floor — the rotated micro-label never shrinks below
/// this, so it stays legible even when a long label wants to overflow the bar.
const LABEL_MIN_PT: f32 = 8.0;

/// The stable per-surface id of a cell, so the app-row layout is addressable —
/// the render + routing are unchanged, but tests can read a cell's rect back via
/// [`egui::Context::read_response`] to click its exact centre (the W10-2 idiom,
/// now that grouping shifts each cell off a hand-computable x).
fn cell_id(surface: Surface) -> egui::Id {
    egui::Id::new(("qbrand-dock-cell", surface))
}

/// The shared point size for every group label — starts at [`Style::SMALL`] and
/// shrinks UNIFORMLY (all six labels together) just enough that the widest label,
/// rotated upright, fits the bar's interior height (its horizontal text width
/// becomes the vertical extent). Floored at [`LABEL_MIN_PT`] for legibility.
fn group_label_font(ui: &egui::Ui, bar_h: f32) -> egui::FontId {
    let base = egui::FontId::proportional(Style::SMALL);
    let widest = ui.fonts(|f| {
        GROUPS
            .iter()
            .map(|g| {
                f.layout_no_wrap(g.label.to_owned(), base.clone(), Style::TEXT)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    });
    let pt = if widest <= bar_h {
        Style::SMALL
    } else {
        (Style::SMALL * bar_h / widest).max(LABEL_MIN_PT)
    };
    egui::FontId::proportional(pt)
}

/// Paint one group's rotated **bottom-to-top** accent label (L1/L4) into a thin
/// column allocated at the current cursor. Display-only (`Sense::hover` — not
/// clickable): after a −90° rotation about its pivot the galley's line height
/// becomes the column width and its text width the vertical extent, dropped so
/// the text reads upward, vertically centred in the bar.
fn group_label(ui: &mut egui::Ui, group: &Group, font: egui::FontId) {
    let galley = ui.fonts(|f| f.layout_no_wrap(group.label.to_owned(), font, group.accent));
    let col_w = galley.size().y;
    let text_w = galley.size().x;
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(col_w, ui.available_height()),
        egui::Sense::hover(),
    );
    // Pivot at the column's left edge; the rotated text spans [pos.y - text_w,
    // pos.y] vertically, so drop the baseline half its width below centre.
    let pos = egui::pos2(rect.left(), rect.center().y + text_w / 2.0);
    ui.painter().add(
        egui::epaint::TextShape::new(pos, galley, group.accent)
            .with_angle(-std::f32::consts::FRAC_PI_2),
    );
}

/// Paint the thin **Carbon-blue** vertical hairline that sits beside a group's
/// label (L3) — the interactive-blue [`Style::ACCENT`] token (§4, not raw hex),
/// inset a hair from the bar's top/bottom edges. Display-only.
fn group_hairline(ui: &mut egui::Ui) {
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(HAIRLINE_W, ui.available_height()),
        egui::Sense::hover(),
    );
    ui.painter().vline(
        rect.center().x,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::ACCENT),
    );
}

/// Render the surface launcher as the full-width bottom **taskbar** into `ui`,
/// selecting the active [`Surface`] and rendering the right-justified status
/// tray (NAVBAR-W10-2). A click on a cell makes that surface active; the
/// active one reads as selected (bottom accent underline + selection wash).
///
/// The layout: the Workbench as a standalone lead, then the six labelled groups
/// (PICKER-1) — each a rotated bottom-to-top accent label + a Carbon-blue
/// hairline before its [`Surface::ALL`]-ordered icon cells — a flexible gap,
/// then the tray (chevron · status icons · clock) against the right edge.
/// Returns `true` when any click routed this frame (a cell or a tray icon) so
/// the shell can surface the body behind a session.
pub fn taskbar(
    ui: &mut egui::Ui,
    active: &mut Surface,
    tray: &mut TrayState,
    inputs: &TrayInputs<'_>,
) -> bool {
    // A hairline top divider on the seam between the surface body above and the
    // bar, drawn from the installed BORDER stroke (a Style token, not a raw
    // colour/width — §4). The flat SURFACE fill is the panel frame (`main.rs`).
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(ui.max_rect().x_range(), ui.max_rect().top(), hairline);

    let mut clicked = false;
    ui.horizontal(|ui| {
        // Cells breathe with a small horizontal gap; each cell still carries its
        // own internal padding around the centred glyph. The same gap spaces the
        // per-group label + hairline from the icons they head.
        ui.spacing_mut().item_spacing = egui::vec2(Style::SP_XS, 0.0);

        // The label micro-type is sized once so all six labels shrink together
        // to fit the bar height (the full row height, before any cell is placed).
        let bar_h = ui.available_height();
        let label_font = group_label_font(ui, bar_h);

        // PICKER-1 — the Workbench leads as the standalone anchor (no group, no
        // label), then the six labelled groups (L5), each in Surface::ALL order
        // within itself (L7). System / Storage / About are ordinary cells inside
        // the System group (the tray still owns the right).
        if cell(ui, Surface::Workbench, active) {
            clicked = true;
        }
        for group in &GROUPS {
            // Generous padding before each group (L3), then the rotated accent
            // label + the Carbon-blue hairline to the LEFT of the icon cells.
            ui.add_space(Style::SP_S);
            group_label(ui, group, label_font.clone());
            group_hairline(ui);
            for &surface in group.surfaces {
                if cell(ui, surface, active) {
                    clicked = true;
                }
            }
        }

        // Lock W2 — flexible space, then the right-justified corner cluster: a
        // right-to-left sub-layout consumes the remaining width, laying out from the
        // RIGHT edge inward in add order. The Win10 "Show Desktop" sliver is added
        // FIRST so it lands right-most (the bottom-right corner, PAST the tray);
        // then the status tray + clock; then the System **Settings** button, which
        // lands just LEFT of the tray — the last app-element before the tray, at the
        // right end of the flexible space (PICKER-2).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            if show_desktop_sliver(ui, active) {
                clicked = true;
            }
            if tray::tray(ui, tray, active, inputs) {
                clicked = true;
            }
            if cell(ui, Surface::System, active) {
                clicked = true;
            }
        });
    });
    clicked
}

/// One ordinary taskbar entry — a full-[`CELL_W`] icon-only glyph cell.
fn cell(ui: &mut egui::Ui, surface: Surface, active: &mut Surface) -> bool {
    launch_cell(ui, surface, active, CELL_W, false)
}

/// The Win10 **"Show Desktop"** sliver — a thin [`SHOW_DESKTOP_W`] button pinned
/// to the bar's far-right corner (rendered right-most in the tray sub-layout, past
/// the tray). A left-edge divider sets it off from the tray (the Win10 corner
/// idiom); it routes to [`Surface::Desktop`] and wears the same active/hover
/// affordances as a cell, just narrower.
fn show_desktop_sliver(ui: &mut egui::Ui, active: &mut Surface) -> bool {
    launch_cell(ui, Surface::Desktop, active, SHOW_DESKTOP_W, true)
}

/// The shared render for a taskbar launch entry (locks W4/W5/W6) — used by both an
/// ordinary [`cell`] and the far-right [`show_desktop_sliver`]: the 24px brand
/// glyph centred in a `width`-wide, full-bar-height column, the accent bottom
/// underline + selection wash when active, a hover fill only (NO tooltip), an
/// optional Win10 left-edge divider (the Show-Desktop sliver), and a click that
/// selects the surface (returned so the shell can surface the body).
fn launch_cell(
    ui: &mut egui::Ui,
    surface: Surface,
    active: &mut Surface,
    width: f32,
    left_divider: bool,
) -> bool {
    let selected = *active == surface;
    // Fill the full bar height so the whole column is clickable. Interact under a
    // stable per-surface id (`cell_id`) so the render + routing are unchanged but
    // the cell's rect is addressable — tests read it back to click its centre now
    // that grouping shifts each cell off a hand-computable x.
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(width, ui.available_height()),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, cell_id(surface), egui::Sense::click());
    let hovered = response.hovered();

    // A painter clone so `egui::Image::paint_at` can still borrow `ui` (splash.rs).
    let painter = ui.painter().clone();

    // Cell background: the selected cell wears the accent selection wash, a
    // hovered one the raised SURFACE_HI — both Style tokens (§4); hover is the
    // fill alone (lock W6 — no tooltip).
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }

    // Active mark (lock W5): the accent underline along the cell's bottom edge.
    if selected {
        painter.rect_filled(underline(rect), egui::CornerRadius::ZERO, Style::ACCENT);
    }

    // The Win10 vertical divider marking the far-right Show-Desktop corner — a
    // BORDER hairline down the sliver's LEFT edge, inset from the bar edges (§4).
    if left_divider {
        painter.vline(
            rect.left(),
            (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
            egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        );
    }

    // Two-tone tint: the active glyph reads solid in the brand ACCENT, a hovered
    // one brightens to full TEXT, the rest sit dim at TEXT_DIM. The brand SVG
    // set is a single `currentColor` variant (no separate outline artwork), so
    // "filled vs outline" is approximated by tint intensity — every value a
    // Style token, never a raw colour (§4).
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

    // The glyph, centred in the cell's BOTTOM icon band (lock W4 — no caption
    // beneath it). The band is the cell minus the TASKBAR_TOP_PAD breathing room,
    // so the glyph sits low (bottom-biased) with clear space above it. A glyph load
    // failure fails soft to the bare cell (§7).
    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), ICON_LOGICAL, tint) {
        let icon_cy = (rect.top() + TASKBAR_TOP_PAD + rect.bottom()) / 2.0;
        let icon_center = egui::pos2(rect.center().x, icon_cy);
        let icon_rect =
            egui::Rect::from_center_size(icon_center, egui::vec2(ICON_LOGICAL, ICON_LOGICAL));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }

    if response.clicked() {
        *active = surface;
        return true;
    }
    false
}

/// Rasterize + upload a brand glyph, cached in egui memory so a given
/// `(glyph, physical-size, tint)` triple is rasterized through `resvg` **once**
/// and then shared as a cheap ref-counted [`TextureHandle`] — never re-rasterized
/// per frame (the backdrop.rs lock-7 pattern). A failed rasterize caches `None`,
/// so a broken asset fails soft (§7) without retrying every frame. Shared with
/// the tray (`tray.rs`), which rasters the 16px tray set through the same cache.
///
/// The glyph is rasterized at the physical pixel size (`logical × ppp`) and drawn
/// back at the logical size, so it stays DPI-crisp at any `HiDPI` scale — the
/// loader honours the exact requested px.
#[allow(
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // size_px ≥ 1.0 by the .max(1.0) clamp
)]
pub fn icon_texture(
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
    use super::{
        cell_id, icon_texture, taskbar, underline, Surface, CELL_W, GROUPS, ICON_LOGICAL,
        SHOW_DESKTOP_W, TASKBAR_H, TASKBAR_TOP_PAD,
    };
    use crate::chrome::MeshSummary;
    use crate::tray::{TrayInputs, TrayState};
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
    fn the_shell_opens_on_the_workbench_surface() {
        assert_eq!(Surface::default(), Surface::Workbench);
    }

    // --- NAVBAR-W10-2: the pixel-per-Win10 metrics + active mark ------------------

    #[test]
    fn the_bar_wears_the_win10_metrics() {
        // Lock W3 @100%: 24px app glyphs and the icon-only cell shrunk to 48px —
        // all on the 8px grid, straight from Style tokens. The bar is the former
        // 40px Win10 height plus an SP_M (16px) top breathing strip = 56px, so the
        // glyphs sit low with clear space above them.
        // 56px = the former 40px Win10 bar + the SP_M (16px) top strip, so the bar
        // is taller than the old 40px for air above the icons.
        assert!((TASKBAR_H - 56.0).abs() < f32::EPSILON, "bar height");
        assert!(
            (TASKBAR_TOP_PAD - 16.0).abs() < f32::EPSILON,
            "top breathing room"
        );
        assert!((ICON_LOGICAL - 24.0).abs() < f32::EPSILON, "app glyph edge");
        assert!((CELL_W - 48.0).abs() < f32::EPSILON, "icon-only cell width");
        // The bar stays on the 8px grid.
        assert!(
            (TASKBAR_H % 8.0).abs() < f32::EPSILON,
            "bar height on the 8px grid"
        );
    }

    #[test]
    fn the_active_underline_hugs_the_cells_bottom_edge() {
        // Lock W5 — the accent mark moved from the cell's top strip to the
        // Win10 bottom-edge underline: full cell width, SP_XS tall, flush with
        // the bottom edge.
        let cell = egui::Rect::from_min_size(
            egui::pos2(96.0, 600.0 - TASKBAR_H),
            egui::vec2(CELL_W, TASKBAR_H),
        );
        let strip = underline(cell);
        assert!(
            (strip.bottom() - cell.bottom()).abs() < f32::EPSILON,
            "flush bottom"
        );
        assert!(
            (strip.height() - Style::SP_XS).abs() < f32::EPSILON,
            "strip height"
        );
        assert!(
            (strip.width() - cell.width()).abs() < f32::EPSILON,
            "full width"
        );
        assert!(
            (strip.left() - cell.left()).abs() < f32::EPSILON,
            "flush left"
        );
        assert!(
            strip.top() > cell.center().y,
            "an underline, not a top strip"
        );
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
            // The System surface is the right-side Settings button — the cog glyph.
            (Surface::System, IconId::Settings),
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
        // tinted by a Style token (no raw hex) — so the bar never draws an empty
        // square.
        let tint = Style::TEXT_DIM.to_array();
        for surface in Surface::ALL {
            let img = icon_image(surface.icon_id(), 32, tint).expect("surface glyph rasterizes");
            let inked = img.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
            assert!(inked > 0, "{surface:?} glyph rasterized empty");
        }
    }

    // --- the bar mounts, renders icon-only, and switches surface on a click -------

    /// Mount the real bottom bar (with a default tray over an unseen mesh) for
    /// one headless frame and return the frame output.
    fn run_taskbar(
        ctx: &egui::Context,
        active: &mut Surface,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mesh = MeshSummary::default();
        let inputs = TrayInputs {
            mesh: &mesh,
            seat: None,
            unread: 0,
            session_active: false,
        };
        let mut tray = TrayState::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::TopBottomPanel::bottom("shell-taskbar")
                .exact_height(TASKBAR_H)
                .frame(egui::Frame::default().fill(Style::SURFACE))
                .show(ctx, |ui| {
                    let _ = taskbar(ui, active, &mut tray, &inputs);
                });
        })
    }

    #[test]
    fn the_taskbar_renders_and_caches_the_glyphs_headless() {
        // Drive one headless frame of the full-width bottom taskbar (the same
        // Context::run → tessellate path the DRM runner uses, minus the GPU): it
        // must draw primitives without panicking, and every surface glyph must
        // resolve to a real texture through the memory-cached loader.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let out = run_taskbar(&ctx, &mut active, Vec::new());
        let ppp = out.pixels_per_point;
        let prims = ctx.tessellate(out.shapes, ppp);
        assert!(!prims.is_empty(), "the taskbar drew nothing");

        for surface in Surface::ALL {
            assert!(
                icon_texture(&ctx, surface.icon_id(), ICON_LOGICAL, Style::TEXT_DIM).is_some(),
                "{surface:?} glyph failed to rasterize + upload"
            );
        }
    }

    /// Collect every text shape's `(angle, fallback_color)` in a frame's output,
    /// recursing into shape groups. The group labels are rotated (angle ≠ 0),
    /// tinted by their group accent; the clock lines are upright (angle 0).
    fn collect_text_shapes(shape: &egui::Shape, out: &mut Vec<(f32, egui::Color32)>) {
        match shape {
            egui::Shape::Text(t) => out.push((t.angle, t.fallback_color)),
            egui::Shape::Vec(v) => {
                for s in v {
                    collect_text_shapes(s, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_bar_shows_group_labels_and_the_clock_no_captions_no_tooltips() {
        // PICKER-1 relayout: the ONLY text on a quiet bar (no unread badge,
        // flyout closed) is the six rotated group labels + the tray clock's two
        // stacked lines — still no per-icon captions (W4) and no tooltips (W6).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let out = run_taskbar(&ctx, &mut active, Vec::new());
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_text_shapes(&clipped.shape, &mut texts);
        }
        assert_eq!(
            texts.len(),
            GROUPS.len() + 2,
            "the quiet bar carries exactly the six group labels + the clock's two lines"
        );
    }

    #[test]
    fn the_group_labels_render_rotated_bottom_to_top_in_their_accent() {
        // L1/L4 — each group's heading is a label rotated 90° CCW (bottom-to-top,
        // angle −π/2) painted in that group's accent colour; the two upright
        // clock lines (angle 0) are the only other text.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::default();
        let out = run_taskbar(&ctx, &mut active, Vec::new());
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_text_shapes(&clipped.shape, &mut texts);
        }
        let rotated: Vec<(f32, egui::Color32)> =
            texts.into_iter().filter(|(a, _)| *a != 0.0).collect();
        assert_eq!(
            rotated.len(),
            GROUPS.len(),
            "one rotated label per group, none for the icons or the clock"
        );
        let accents: Vec<egui::Color32> = GROUPS.iter().map(|g| g.accent).collect();
        for (angle, color) in rotated {
            assert!(
                (angle - (-std::f32::consts::FRAC_PI_2)).abs() < 1e-3,
                "label reads bottom-to-top (−π/2), got {angle}"
            );
            assert!(
                accents.contains(&color),
                "label painted in a group accent, got {color:?}"
            );
        }
    }

    #[test]
    fn clicking_a_taskbar_cell_selects_that_surface() {
        // The click→select behaviour survives the icon-only relayout. Mount the
        // real bottom bar and click the leftmost cell (Workbench, the nav head).
        // egui hit-tests a press against the settled widget rects, so prime a
        // couple of no-event frames first, then press one frame + release the
        // next (the egui click model), and the active surface follows the click.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::About;
        // The first cell is flush-left and fills the bar's height, so its centre
        // — half a cell in, half the bar up from the bottom — is the retargeted
        // click point (derived from the layout constants, not a magic number).
        let click = egui::pos2(CELL_W / 2.0, 600.0 - TASKBAR_H / 2.0);
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
        let _ = run_taskbar(&ctx, &mut active, Vec::new());
        let _ = run_taskbar(&ctx, &mut active, Vec::new());
        // Move onto the Workbench cell + press, then release the next frame.
        let _ = run_taskbar(
            &ctx,
            &mut active,
            vec![egui::Event::PointerMoved(click), press],
        );
        let _ = run_taskbar(&ctx, &mut active, vec![release]);
        assert_eq!(
            active,
            Surface::Workbench,
            "clicking the Workbench cell selected it"
        );
    }

    // --- PICKER-1: the group table + rotated labels + hairline dividers -----------

    #[test]
    fn the_locked_group_taxonomy_and_order() {
        // L5/L7 — six groups in the locked left-to-right order, each listing its
        // surfaces in Surface::ALL relative order; About lives in the System group.
        // THREE surfaces are in no group: the Workbench (standalone lead), the
        // System surface (right-side Settings button), and Desktop (far-right
        // Show-Desktop sliver).
        use Surface::{
            About, Browser, Chat, Editor, Files, Instances, Media, MeshView, Music, Storage,
            Terminal, Voice, Workbench,
        };
        let expect: [(&str, &[Surface]); 6] = [
            ("Comms", &[Voice, Chat]),
            ("Workloads", &[Instances]),
            ("Terminals", &[Browser, Terminal, Editor]),
            ("Mesh", &[MeshView]),
            ("System", &[Files, Storage, About]),
            ("Media", &[Music, Media]),
        ];
        assert_eq!(GROUPS.len(), expect.len(), "six groups");
        for (g, (label, surfaces)) in GROUPS.iter().zip(expect) {
            assert_eq!(g.label, label, "group order");
            assert_eq!(
                g.surfaces, surfaces,
                "{label} membership + within-group order"
            );
        }
        let system = GROUPS.iter().find(|g| g.label == "System").unwrap();
        assert!(
            system.surfaces.contains(&About),
            "About lives in the System group"
        );
        // The three ungrouped surfaces are placed by the lead / the Settings button
        // / the far-right sliver, never a group.
        for ungrouped in [Workbench, Surface::System, Surface::Desktop] {
            assert!(
                GROUPS.iter().all(|g| !g.surfaces.contains(&ungrouped)),
                "{ungrouped:?} is placed outside every group"
            );
        }
    }

    #[test]
    fn each_group_takes_its_shared_style_accent_token() {
        // PICKER-2: the group labels are keyed by the shared categorical tokens on
        // `mde_egui::Style` (the SAME six EXPLORER-15 consumes for category identity,
        // design O8) — defined once, consumed here. No local placeholder hex survives.
        let expect: [(&str, egui::Color32); 6] = [
            ("Comms", Style::ACCENT_COMMS),
            ("Workloads", Style::ACCENT_WORKLOADS),
            ("Terminals", Style::ACCENT_TERMINALS),
            ("Mesh", Style::ACCENT_MESH),
            ("System", Style::ACCENT_SYSTEM),
            ("Media", Style::ACCENT_MEDIA),
        ];
        for (g, (label, token)) in GROUPS.iter().zip(expect) {
            assert_eq!(g.label, label, "group order");
            assert_eq!(
                g.accent, token,
                "{label} label takes its shared Style token"
            );
        }
    }

    #[test]
    fn the_groups_cover_every_surface_once_in_surface_all_order() {
        // The Workbench lead + the System Settings button + the far-right Desktop
        // sliver + the six groups reproduce all 15 of Surface::ALL, each surface
        // placed exactly once...
        let mut placed: Vec<Surface> = vec![Surface::Workbench, Surface::System, Surface::Desktop];
        for g in &GROUPS {
            placed.extend_from_slice(g.surfaces);
        }
        assert_eq!(
            placed.len(),
            Surface::ALL.len(),
            "every surface placed once"
        );
        for s in Surface::ALL {
            assert_eq!(
                placed.iter().filter(|&&x| x == s).count(),
                1,
                "{s:?} appears once across the lead + Settings + the Desktop sliver + groups"
            );
        }
        // ...and L7: within each group the surfaces keep Surface::ALL relative
        // order (their ALL indices ascend).
        let idx = |s: Surface| Surface::ALL.iter().position(|&x| x == s).unwrap();
        for g in &GROUPS {
            let idxs: Vec<usize> = g.surfaces.iter().map(|&s| idx(s)).collect();
            assert!(
                idxs.is_sorted(),
                "group {} keeps Surface::ALL order",
                g.label
            );
        }
    }

    #[test]
    fn clicking_any_group_cell_routes_to_its_surface() {
        // §7 — every one of the 15 surfaces still routes on a click after the
        // grouping relayout (Workbench lead + all cells in the six groups). Mount
        // the real bar, read each cell's settled rect by its stable id, then click
        // its exact centre (the W10-2 idiom) and assert the active surface follows.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut warm = Surface::Workbench;
        // Prime two frames so every cell rect is registered under its id.
        let _ = run_taskbar(&ctx, &mut warm, Vec::new());
        let _ = run_taskbar(&ctx, &mut warm, Vec::new());
        let mut centers: Vec<(Surface, egui::Pos2)> = Vec::new();
        for s in Surface::ALL {
            let response = ctx.read_response(cell_id(s));
            assert!(response.is_some(), "{s:?} cell rect not registered");
            let rect = response.expect("registered above").rect;
            centers.push((s, rect.center()));
        }
        for (want, center) in centers {
            // Start on a different surface so the click is observable.
            let mut active = if want == Surface::Workbench {
                Surface::About
            } else {
                Surface::Workbench
            };
            let press = egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            };
            let release = egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            };
            let _ = run_taskbar(
                &ctx,
                &mut active,
                vec![egui::Event::PointerMoved(center), press],
            );
            let _ = run_taskbar(&ctx, &mut active, vec![release]);
            assert_eq!(active, want, "clicking {want:?}'s cell selects it");
        }
    }

    #[test]
    fn the_desktop_sliver_pins_to_the_far_right_corner_past_the_tray() {
        // The Win10 "Show Desktop" move: Desktop is NOT a group cell — it renders as
        // a thin sliver held in the bottom-right corner, right-most on the whole bar
        // (past the tray). Mount the real bar, settle the layout, and read the
        // Desktop cell rect back: its right edge hugs the bar's right edge (nothing
        // sits further right — i.e. past the tray) and it is narrower than a normal
        // cell (a sliver, SHOW_DESKTOP_W).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::Workbench;
        let _ = run_taskbar(&ctx, &mut active, Vec::new());
        let _ = run_taskbar(&ctx, &mut active, Vec::new());

        let desktop = ctx
            .read_response(cell_id(Surface::Desktop))
            .expect("the Desktop sliver rect is registered")
            .rect;
        // The bar spans the 1280-wide screen (run_taskbar's screen_rect); the sliver
        // hugs its right edge — the far-right corner, past the tray.
        assert!(
            (desktop.right() - 1280.0).abs() < 1.0,
            "the Desktop sliver hugs the bar's right edge, got right={}",
            desktop.right()
        );
        // Every group cell sits to its LEFT — nothing renders further right.
        for s in Surface::ALL {
            if s == Surface::Desktop {
                continue;
            }
            if let Some(resp) = ctx.read_response(cell_id(s)) {
                assert!(
                    resp.rect.right() <= desktop.right() + f32::EPSILON,
                    "{s:?} renders to the right of the Desktop sliver"
                );
            }
        }
        // It is a thin sliver — narrower than a normal cell.
        assert!(
            (desktop.width() - SHOW_DESKTOP_W).abs() < f32::EPSILON,
            "the Desktop sliver is SHOW_DESKTOP_W wide"
        );
        assert!(
            desktop.width() < CELL_W,
            "the Desktop sliver is narrower than a normal cell"
        );
    }
}
