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
//!
//! **VDOCK-1** adds the left **vertical dock** ([`dock`], design
//! `docs/design/vertical-dock.md`) in parallel: a left-edge, full-height, ~48px
//! slide-in auto-hide column that will REPLACE this bottom [`taskbar`]. VDOCK-1
//! builds only its frame + auto-hide (Super-tap toggle + pin + slide); the app
//! picker / status quads / system quad land in VDOCK-2/3/4. The shell mounts one
//! or the other via a flag (default the vertical dock); this `taskbar` stays
//! intact until VDOCK-6 rips it out at the cutover.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::{Motion, Style};
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

// ── PICKER-3: the group's spacing rhythm (8px grid; §4 — no raw px) ───────────
// Every horizontal gap in the grouped run is added EXPLICITLY from these three
// tokens (the automatic item-spacing is zeroed in `taskbar`), so the rhythm is
// even + measurable: mixing an auto per-item gap with a manual `add_space` is
// what left the labels cramped against their hairline/icons and the group
// boundaries reading unevenly. The three gaps form a clear hierarchy —
// `GROUP_GAP`(16) ≫ `LABEL_PAD`(8) > `ICON_GAP`(4) — so a group reads as one
// cluster set clearly apart from the next.

/// The generous inter-group separation — the clear gap BEFORE each rotated accent
/// label (and before the first group, off the Workbench lead). `SP_M`.
const GROUP_GAP: f32 = Style::SP_M;

/// The small breathing pad on EACH side of a group's Carbon-blue hairline
/// (label → pad → hairline → pad → icons), so the rotated label never crowds the
/// rule and the rule never crowds the icons. `SP_S`.
const LABEL_PAD: f32 = Style::SP_S;

/// The tight gap between the icon cells WITHIN one group cluster. `SP_XS`.
const ICON_GAP: f32 = Style::SP_XS;

/// The stable per-surface id of a cell, so the app-row layout is addressable —
/// the render + routing are unchanged, but tests can read a cell's rect back via
/// [`egui::Context::read_response`] to click its exact centre (the W10-2 idiom,
/// now that grouping shifts each cell off a hand-computable x).
fn cell_id(surface: Surface) -> egui::Id {
    egui::Id::new(("qbrand-dock-cell", surface))
}

/// The stable id of a group's rotated label column, so the app-row layout is fully
/// addressable — the render is unchanged (the label is display-only, `Sense::hover`),
/// but the layout harness can read its settled `Rect` back to measure the group's
/// spacing rhythm (PICKER-3).
fn group_label_id(label: &str) -> egui::Id {
    egui::Id::new(("qbrand-dock-group-label", label))
}

/// The stable id of a group's Carbon-blue hairline rule, so the harness can read
/// its settled `Rect` (its x + vertical extent) back. Display-only.
fn group_hairline_id(label: &str) -> egui::Id {
    egui::Id::new(("qbrand-dock-group-hairline", label))
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
    // Register the settled column under a stable id so the harness can read its
    // rect back (still display-only — hover sense, no click, nothing painted here).
    ui.interact(rect, group_label_id(group.label), egui::Sense::hover());
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
fn group_hairline(ui: &mut egui::Ui, group: &Group) {
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(HAIRLINE_W, ui.available_height()),
        egui::Sense::hover(),
    );
    // Register the settled rule under a stable id (display-only) so the harness can
    // measure the label→hairline→icon rhythm and the cross-group alignment.
    ui.interact(rect, group_hairline_id(group.label), egui::Sense::hover());
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
        // Every horizontal gap in the grouped run is added EXPLICITLY below (from
        // the GROUP_GAP / LABEL_PAD / ICON_GAP tokens), so zero the automatic
        // item-spacing: mixing an auto per-item gap with a manual `add_space` is
        // what left the labels cramped and the group boundaries reading unevenly.
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

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
            // PICKER-3 — one even, generous rhythm per group: a clear GROUP_GAP
            // before the rotated accent label, a LABEL_PAD off the Carbon-blue
            // hairline (L3), the hairline, another LABEL_PAD, then the icon cells
            // clustered ICON_GAP apart. Every gap is a Style token (§4, no raw px).
            ui.add_space(GROUP_GAP);
            group_label(ui, group, label_font.clone());
            ui.add_space(LABEL_PAD);
            group_hairline(ui, group);
            ui.add_space(LABEL_PAD);
            for (i, &surface) in group.surfaces.iter().enumerate() {
                if i > 0 {
                    ui.add_space(ICON_GAP);
                }
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

// ═══════════════════════════════════════════════════════════════════════════
// VDOCK-1 — the left **vertical dock** frame + auto-hide (design
// `docs/design/vertical-dock.md`, locks #1/#9/#13/#14/#23/#24).
//
// The eventual replacement for the horizontal [`taskbar`] above: a left-edge,
// full-height, ~48px, solid Carbon-dark column that slides in from the left and
// auto-hides (hotkey + pin, no hover). VDOCK-1 builds ONLY the FRAME + the
// slide/toggle/pin mechanism; the interior stays three empty seams for the
// follow-ups (app picker VDOCK-2, status quads VDOCK-3, system quad VDOCK-4). It
// mounts in parallel with the still-intact `taskbar` — the shell picks one via a
// flag (default the vertical dock); VDOCK-6 rips the bottom bar out at the cutover.
// ═══════════════════════════════════════════════════════════════════════════

/// The vertical dock's width in logical points (~48px, design #2/#23) — one
/// column, the SAME 48px module as the horizontal taskbar's icon cell
/// ([`CELL_W`]), so VDOCK-2's app glyphs + VDOCK-3/4's quads inherit the grid.
/// (`pub`, not `pub(crate)` — the `clippy::redundant_pub_crate` form for a
/// crate-visible item in a private module, like [`TASKBAR_H`].)
pub const DOCK_W: f32 = CELL_W;

/// The egui memory key for the dock's slide animation (the Motion latch that
/// eases the reveal 0↔1). Private to the dock.
const DOCK_SLIDE_KEY: &str = "vdock-slide";

/// The stable id of the dock's floating [`egui::Area`] layer, so the shell (and
/// the passthrough test) can name its `LayerId` — `LayerId::new(Foreground,
/// Id::new(DOCK_AREA))`.
const DOCK_AREA: &str = "vdock-area";

/// The left vertical dock's **state** — VDOCK-1's auto-hide inputs (locks #9/#13)
/// plus VDOCK-2's picker state. The auto-hide half (the Super-tap **reveal** latch
/// and the **pin**) is kept tiny and pure (no egui, no GPU) so the shell's hotkey
/// path toggles it and the render reads [`Self::shown`] headless-testably; there
/// is deliberately **no hover-reveal** (lock #9). VDOCK-2 adds the picker's
/// `active` surface (the shell body follows it, carried over from the horizontal
/// bar's routing) and the `overflow_open` popup latch (#22). The shell-side getter
/// that reads `active` back into the central view lands with the `main.rs` wire,
/// out of this unit's dock.rs-only fence.
#[derive(Debug, Default)]
pub struct DockState {
    /// Toggled by a clean Super tap (lock #13) — the hotkey reveal/hide latch.
    revealed: bool,
    /// The pin (lock #9): while set, the dock stays on screen regardless of the
    /// reveal latch — the "hotkey + pin" hold-open.
    pinned: bool,
    /// The **active surface** the app picker selects (VDOCK-2) — a picker cell
    /// click writes it here; the shell body follows [`Self::active`]. Defaults to
    /// [`Surface::Workbench`] (the shell opens on the Workbench).
    active: Surface,
    /// Whether the '…' **overflow** more-popup is open (VDOCK-2, lock #22) — set by
    /// the '…' cell, cleared on a route or a click-away.
    overflow_open: bool,
}

impl DockState {
    /// Toggle the Super-tap **reveal** — the VDOCK-1 hotkey path calls this on a
    /// clean Super tap (`hotkeys::HotkeyRouter::take_dock_toggle`). A pinned dock
    /// stays shown regardless — see [`Self::shown`].
    pub const fn toggle(&mut self) {
        self.revealed = !self.revealed;
    }

    /// Whether the dock should be on screen this frame: revealed **or** pinned
    /// (the pin holds it open, lock #9).
    pub const fn shown(&self) -> bool {
        self.revealed || self.pinned
    }

    /// Whether the dock is pinned open.
    pub const fn pinned(&self) -> bool {
        self.pinned
    }

    /// Flip the **pin** (the in-dock pin toggle). Pinning also reveals, so the
    /// dock never animates out from under a just-set pin; unpinning leaves the
    /// reveal latch as it was (a Super tap then hides it).
    pub const fn toggle_pin(&mut self) {
        self.pinned = !self.pinned;
        if self.pinned {
            self.revealed = true;
        }
    }
}

/// Render the **left vertical dock** (VDOCK-1) — the slide-in, auto-hide chrome
/// that will replace the bottom [`taskbar`]. A left-edge, full-height [`DOCK_W`]
/// column: a solid Carbon-dark panel with a hairline right-edge divider (locks
/// #1/#24, §4 tokens). Hidden off the left by default; the shell's Super-tap
/// toggles it and the pin holds it open (`state`), sliding in/out from the left
/// edge over the shared [`Motion`] table (~200ms, locks #13/#14).
///
/// Mounted as a floating [`egui::Area`] (NOT a [`egui::SidePanel`]) so it reserves
/// **no gutter** — the central surface fills the full width whether the dock is in
/// or out. When fully hidden **and settled** it mounts **no layer at all**, so
/// egui hit-tests every pointer/key event straight to the surface beneath: the
/// dock can never steal focus/input while hidden (the design's "auto-hide + DRM
/// seat" risk; proven by the passthrough test).
///
/// VDOCK-1 built the FRAME + the slide/toggle/pin; **VDOCK-2** fills the top
/// **Workbench-lead** zone + the single-column **app-groups** middle
/// ([`paint_dock_frame`]). The bottom **status + system quad** zone stays a seam
/// for VDOCK-3/4. Returns `true` if a dock control routed this frame — the pin, or
/// a picker cell selecting its [`Surface`] (recorded in [`DockState`]'s active
/// surface, which the shell reads back to surface the body).
pub fn dock(ctx: &egui::Context, state: &mut DockState) -> bool {
    let shown = state.shown();
    // Slide-in-from-left over the shared Motion table (lock #14): `t` eases
    // 0 (fully hidden, off the left edge) → 1 (fully in, flush at x=0).
    let t = Motion::animate(ctx, DOCK_SLIDE_KEY, shown, Motion::BASE);

    // Fully hidden + settled → mount NO layer. With no Area over the left edge,
    // egui's hit-test routes every pointer/key event to the surface beneath (the
    // background CentralPanel), so the hidden dock steals nothing (lock #9, the
    // DRM-seat passthrough guarantee). The slide-out's final frame lands here once
    // `t` decays to ~0.
    if t <= 0.001 {
        return false;
    }

    let screen = ctx.screen_rect();
    // The slide offset: the panel's left edge rides from -DOCK_W (fully out) to 0
    // (fully in). `constrain(false)` below lets the Area sit at negative x.
    let offset_x = -(1.0 - t) * DOCK_W;

    let mut clicked = false;
    egui::Area::new(egui::Id::new(DOCK_AREA))
        .order(egui::Order::Foreground)
        // It SLIDES (lock #14) — never egui's default fade-in.
        .fade_in(false)
        // Allow the negative-x off-screen slide (the Area is constrained to the
        // screen rect by default, which would clamp the slide to x=0).
        .constrain(false)
        .fixed_pos(egui::pos2(offset_x, screen.top()))
        .show(ctx, |ui| {
            // Claim the full-height column as the Area's content rect, so while the
            // dock is visible its layer covers the whole column (egui routes clicks
            // over it to the dock, not the surface behind). Off-screen portions of
            // the claim simply can't be hit; the fully-hidden case returned above.
            let (rect, _claim) =
                ui.allocate_exact_size(egui::vec2(DOCK_W, screen.height()), egui::Sense::hover());
            clicked = paint_dock_frame(ui, rect, state);
        });

    // Keep frames flowing while the slide is in flight so the motion is smooth
    // (the curtain's tween idiom) — a no-op once settled at either end.
    if t > 0.001 && t < 0.999 {
        ctx.request_repaint();
    }
    clicked
}

/// Paint the vertical dock's frame into `rect` and lay out its interior: the solid
/// Carbon-dark panel + the hairline right-edge divider (lock #24, §4 tokens), the
/// **VDOCK-2** top zone (the Workbench lead + the folded-in pin) and middle zone
/// (the single-column app groups + '…' overflow), and the bottom seam VDOCK-3/4
/// fills. Returns `true` if the pin or a picker cell routed this frame.
fn paint_dock_frame(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let painter = ui.painter().clone();
    // Solid Carbon-dark panel fill (lock #24) — the SURFACE token (§4), the same
    // flat fill the horizontal bar wears, so the two docks read as one chrome.
    painter.rect_filled(rect, egui::CornerRadius::ZERO, Style::SURFACE);
    // The hairline right-edge divider (lock #24) — a 1px BORDER rule down the
    // dock's right edge, the seam between the dock and the surface it floats over.
    painter.vline(
        rect.right(),
        rect.y_range(),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );

    let mut clicked = false;

    // ── TOP zone (design #8) — the Workbench lead pinned top, the pin folded in ──
    // The Workbench lead is the topmost cell (the mesh-control home, always one
    // click away); VDOCK-1's pin toggle (lock #9 — the "pin" half of "hotkey +
    // pin") folds into a slim strip just beneath it. A BORDER hairline sets the
    // lead apart from the app groups below.
    let wb = egui::Rect::from_min_size(rect.min, egui::vec2(DOCK_W, DOCK_W));
    if pick_app_cell(ui, Surface::Workbench, &mut state.active, wb) {
        clicked = true;
    }
    let pin = egui::Rect::from_min_size(
        egui::pos2(rect.left(), wb.bottom()),
        egui::vec2(DOCK_W, PIN_STRIP_H),
    );
    if pin_toggle(ui, pin, state) {
        clicked = true;
    }
    painter.hline(
        (rect.left() + Style::SP_XS)..=(rect.right() - Style::SP_XS),
        pin.bottom() + GROUP_DIVIDER_H / 2.0,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );

    // ── MIDDLE zone (design #2/#3/#4/#10/#11/#21/#22/#23) — the app picker ──
    // The six labelled groups stacked single-column top→bottom (Comms → … →
    // Media), each a horizontal accent label (#4) + a left-rail accent stripe +
    // accent divider (#21) over its icon-only 24px cells (#11/#23) in Surface::ALL
    // order (#3). The zone is bounded above the BOTTOM_ZONE_H band reserved for
    // VDOCK-3/4's status + system quads; groups that overrun it fold into the '…'
    // more-popup (#22).
    let middle_top = pin.bottom() + GROUP_DIVIDER_H;
    let middle_bottom = rect.bottom() - BOTTOM_ZONE_H;
    let middle_h = (middle_bottom - middle_top).max(0.0);
    let visible = visible_group_count(middle_h);
    // Fit the labels to the column interior — the full width less an SP_XS side
    // margin each side (SP_XS + SP_XS = SP_S total).
    let font = group_label_font(ui, DOCK_W - Style::SP_S);
    let mut y = middle_top;
    for group in &GROUPS[..visible] {
        let (h, routed) = pick_group(
            ui,
            group,
            egui::pos2(rect.left(), y),
            DOCK_W,
            &font,
            &mut state.active,
        );
        if routed {
            clicked = true;
        }
        y += h;
    }
    if visible < GROUPS.len() && pick_overflow(ui, rect, middle_bottom, visible, &font, state) {
        clicked = true;
    }

    // ── BOTTOM zone (SEAM → VDOCK-3/4) — the last BOTTOM_ZONE_H of the column is
    // reserved for the stacked 2×2 status quads (Chat/BT/Vol/Batt · Status/Signal/
    // Peers/Sessions) + the system quad (Settings · Show-Desktop · Lock · Power);
    // VDOCK-2 bounds the app zone above it and leaves it empty (the frame fill
    // shows through) — VDOCK-3/4 fill it.
    clicked
}

/// The dock's **pin** toggle (VDOCK-1, lock #9) — the minimal affordance that
/// holds the dock open when set (the "pin" half of "hotkey + pin, no hover").
/// The brand set has no pin glyph yet (VDOCK-4 gives the dock its real glyphs), so
/// this is a small centred dot: a filled ACCENT disc when pinned, a dim ring when
/// not (a hover brightens it). Every colour is a Style token (§4). Returns `true`
/// on a click (which flips the pin via [`DockState::toggle_pin`]).
fn pin_toggle(ui: &egui::Ui, cell: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(cell, egui::Id::new("vdock-pin"), egui::Sense::click());
    let pinned = state.pinned();
    let color = if pinned {
        Style::ACCENT
    } else if resp.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let r = Style::SP_S / 2.0;
    if pinned {
        ui.painter().circle_filled(cell.center(), r, color);
    } else {
        ui.painter()
            .circle_stroke(cell.center(), r, egui::Stroke::new(HAIRLINE_W, color));
    }
    if resp.clicked() {
        state.toggle_pin();
        return true;
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// VDOCK-2 — the vertical **app picker** (fills the dock's TOP + MIDDLE zones;
// design `docs/design/vertical-dock.md`, locks #2/#3/#4/#10/#11/#21/#22/#23).
//
// The Workbench lead (top) + the six labelled app groups stacked single-column in
// the middle — each with a horizontal accent label (#4), a left-rail accent stripe
// + accent divider (#21), icon-only 24px cells (#11/#23), a left-edge active bar
// (#10), and a '…' more-popup when the groups overrun the zone (#22). The picker's
// GROUPS + surface→glyph map + accent tokens all carry over from the horizontal
// `taskbar` (this is a re-layout, not a rebuild). Settings (the System surface) +
// Show-Desktop are NOT in the picker — they belong to VDOCK-4's bottom system
// quad; every other surface appears here exactly once (the same union the
// compile-time guard above pins). A click routes into `DockState::active`.
// ═══════════════════════════════════════════════════════════════════════════

/// The single-column app-cell height (design #23) — a 24px glyph ([`ICON_LOGICAL`])
/// centred in an [`Style::SP_XL`]-tall cell, on the 8px grid.
const APP_CELL_H: f32 = Style::SP_XL;

/// The slim **pin** strip beneath the Workbench lead (lock #9) — folds VDOCK-1's
/// pin toggle in just under the lead glyph. `SP_M` tall.
const PIN_STRIP_H: f32 = Style::SP_M;

/// The horizontal accent-label row above each group (#4) — `SP_M` tall, its label
/// sized to fit the narrow column by [`group_label_font`].
const PICK_LABEL_H: f32 = Style::SP_M;

/// The per-group accent **divider** band (#21) — an `SP_S` gap with the group's
/// accent hairline centred in it, separating one group from the next.
const GROUP_DIVIDER_H: f32 = Style::SP_S;

/// The thin **left-rail accent stripe** beside each group's cells (#21) — a 2px
/// group-accent spine (twice the [`HAIRLINE_W`] rule), inset [`Style::SP_XS`] from
/// the column's left edge.
const RAIL_W: f32 = HAIRLINE_W * 2.0;

/// The active cell's **left-edge accent bar** (lock #10) — an `SP_XS`-wide
/// [`Style::ACCENT`] bar down the active surface's left edge (the vertical analog
/// of the horizontal bar's bottom underline), at the cell's outer edge.
const ACTIVE_BAR_W: f32 = Style::SP_XS;

/// The '…' overflow cell height (#22) — the more-popup trigger at the bottom of
/// the app zone. `SP_L`.
const OVERFLOW_H: f32 = Style::SP_L;

/// The bottom band reserved for VDOCK-3/4's stacked status quads + system quad
/// (design #8) — three quad rows (~`DOCK_W` each). VDOCK-2 bounds the middle app
/// zone above it and leaves it empty; sizing the middle against this reserve makes
/// the '…' overflow (#22) real on a short screen. VDOCK-3/4 fill the band.
const BOTTOM_ZONE_H: f32 = 3.0 * DOCK_W;

/// The stable per-surface id of a vertical-picker cell — the render + routing are
/// unchanged, but tests read a cell's settled `Rect` back to click its exact
/// centre (the taskbar [`cell_id`] idiom, kept distinct so the two docks' cells
/// never share an id).
fn pick_cell_id(surface: Surface) -> egui::Id {
    egui::Id::new(("vdock-pick-cell", surface))
}

/// The stable id of a group's horizontal accent label, so the harness can read its
/// settled `Rect` back. Display-only (hover sense).
fn pick_label_id(label: &str) -> egui::Id {
    egui::Id::new(("vdock-pick-label", label))
}

/// The stable id of the '…' overflow cell (#22).
fn overflow_more_id() -> egui::Id {
    egui::Id::new("vdock-pick-overflow")
}

/// The rendered height of one group in the app zone: its accent label row + its
/// single-column cells + its accent divider band.
#[allow(
    clippy::cast_precision_loss, // surface counts are tiny (≤3)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn group_height(group: &Group) -> f32 {
    PICK_LABEL_H + group.surfaces.len() as f32 * APP_CELL_H + GROUP_DIVIDER_H
}

/// How many of the six [`GROUPS`] fit, top→down, in a `middle_h`-tall app zone
/// (#22). If they all fit, all six render inline; otherwise the zone reserves its
/// bottom [`OVERFLOW_H`] for the '…' cell and fits as many *whole* groups above it
/// as it can — the rest fold into the more-popup.
fn visible_group_count(middle_h: f32) -> usize {
    let total: f32 = GROUPS.iter().map(group_height).sum();
    if total <= middle_h {
        return GROUPS.len();
    }
    let avail = (middle_h - OVERFLOW_H).max(0.0);
    let mut used = 0.0;
    let mut n = 0;
    for group in &GROUPS {
        let h = group_height(group);
        if used + h > avail {
            break;
        }
        used += h;
        n += 1;
    }
    n
}

/// One single-column **app cell** (#2/#11/#23) — a 24px brand glyph centred in a
/// `width`-wide × [`APP_CELL_H`]-tall cell, icon-only (no tooltip, #11). The active
/// surface wears the left-edge [`Style::ACCENT`] bar (#10) + the subtle selection
/// wash; a hover is a fill only. A click routes to the surface (sets `active`,
/// returns `true` so the shell can surface the body). Every colour is a Style
/// token (§4); shared by the Workbench lead, the middle groups, and the '…' popup.
fn pick_app_cell(ui: &egui::Ui, surface: Surface, active: &mut Surface, rect: egui::Rect) -> bool {
    let selected = *active == surface;
    let resp = ui.interact(rect, pick_cell_id(surface), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();

    // Subtle fill: the selected cell wears the accent selection wash, a hovered one
    // the raised SURFACE_HI (both Style tokens, §4).
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }

    // Active mark (lock #10): the accent bar down the cell's LEFT edge.
    if selected {
        let bar =
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(ACTIVE_BAR_W, rect.height()));
        painter.rect_filled(bar, egui::CornerRadius::ZERO, Style::ACCENT);
    }

    // Two-tone tint (the taskbar idiom): active reads solid ACCENT, a hovered one
    // brightens to full TEXT, the rest sit dim at TEXT_DIM — the glyph is tinted at
    // rasterization, so it's blitted with WHITE (no extra multiply). A load failure
    // fails soft to the bare cell (§7).
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), ICON_LOGICAL, tint) {
        let icon =
            egui::Rect::from_center_size(rect.center(), egui::vec2(ICON_LOGICAL, ICON_LOGICAL));
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }

    if resp.clicked() {
        *active = surface;
        return true;
    }
    false
}

/// Paint one **group** (#3/#4/#21) into the column at `origin`, `width` wide: the
/// horizontal accent label (#4), the single-column icon cells ([`Surface::ALL`]
/// order), the left-rail accent stripe beside them (#21), and the accent divider
/// (#21). Returns `(height, routed)` — the consumed height + whether a cell routed
/// this frame. Shared by the middle zone and the '…' overflow popup.
#[allow(
    clippy::cast_precision_loss, // per-group cell counts are tiny (≤3)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn pick_group(
    ui: &egui::Ui,
    group: &Group,
    origin: egui::Pos2,
    width: f32,
    font: &egui::FontId,
    active: &mut Surface,
) -> (f32, bool) {
    let painter = ui.painter().clone();

    // The horizontal accent label above the group (#4) — display-only (hover sense)
    // so the harness can read its rect; painted in the group accent, centred.
    let label_rect = egui::Rect::from_min_size(origin, egui::vec2(width, PICK_LABEL_H));
    ui.interact(label_rect, pick_label_id(group.label), egui::Sense::hover());
    let galley =
        ui.fonts(|f| f.layout_no_wrap(group.label.to_owned(), font.clone(), group.accent));
    let lp = egui::pos2(
        label_rect.center().x - galley.size().x / 2.0,
        label_rect.center().y - galley.size().y / 2.0,
    );
    painter.galley(lp, galley, group.accent);

    // The single-column icon cells (#2), stacked under the label in Surface::ALL
    // order (#3/L7).
    let cells_top = label_rect.bottom();
    let mut routed = false;
    for (i, &surface) in group.surfaces.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(origin.x, cells_top + i as f32 * APP_CELL_H),
            egui::vec2(width, APP_CELL_H),
        );
        if pick_app_cell(ui, surface, active, cell) {
            routed = true;
        }
    }
    let cells_bottom = cells_top + group.surfaces.len() as f32 * APP_CELL_H;

    // The thin left-rail accent stripe beside the cells (#21) — the group's spine,
    // painted over the cell fills in the group accent, inset SP_XS from the edge.
    let rail = egui::Rect::from_min_max(
        egui::pos2(origin.x + Style::SP_XS, cells_top),
        egui::pos2(origin.x + Style::SP_XS + RAIL_W, cells_bottom),
    );
    painter.rect_filled(rail, egui::CornerRadius::ZERO, group.accent);

    // The accent divider below the group (#21).
    painter.hline(
        (origin.x + Style::SP_XS)..=(origin.x + width - Style::SP_XS),
        cells_bottom + GROUP_DIVIDER_H / 2.0,
        egui::Stroke::new(HAIRLINE_W, group.accent),
    );

    (group_height(group), routed)
}

/// The '…' **more-popup** overflow (lock #22) — when the groups overrun the app
/// zone, a '…' cell at the zone's bottom toggles a floating popup of the hidden
/// groups (label + cells), each routing on click. Returns `true` when a popup cell
/// routed this frame. Chosen over a scrollbar because a scrollbar would eat the
/// 48px column's width; the '…' popup keeps the picker icon-clean.
fn pick_overflow(
    ui: &egui::Ui,
    rect: egui::Rect,
    middle_bottom: f32,
    visible: usize,
    font: &egui::FontId,
    state: &mut DockState,
) -> bool {
    let more = egui::Rect::from_min_size(
        egui::pos2(rect.left(), middle_bottom - OVERFLOW_H),
        egui::vec2(DOCK_W, OVERFLOW_H),
    );
    let resp = ui.interact(more, overflow_more_id(), egui::Sense::click());
    let opened = resp.clicked();
    if opened {
        state.overflow_open = !state.overflow_open;
    }
    // The '…' glyph — brightens on hover / while the popup is open (Style tokens).
    let color = if state.overflow_open || resp.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let dots = ui.fonts(|f| {
        f.layout_no_wrap("…".to_owned(), egui::FontId::proportional(Style::BODY), color)
    });
    ui.painter().galley(
        egui::pos2(
            more.center().x - dots.size().x / 2.0,
            more.center().y - dots.size().y / 2.0,
        ),
        dots,
        color,
    );

    if !state.overflow_open {
        return false;
    }

    // The floating popup of the hidden groups — anchored to the right of the '…'
    // cell and growing upward (the tray flyout idiom): a SURFACE panel + hairline
    // border behind the same single-column groups.
    let hidden = &GROUPS[visible..];
    let popup_h: f32 = hidden.iter().map(group_height).sum();
    let inner = egui::Area::new(egui::Id::new("vdock-overflow-popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(more.right() + Style::SP_XS, more.bottom()))
        .show(ui.ctx(), |ui| {
            let (area, _) =
                ui.allocate_exact_size(egui::vec2(DOCK_W, popup_h), egui::Sense::hover());
            // Reserve a slot so the panel background paints BEHIND the cells (the
            // tray/keyboard overlay idiom).
            let bg = ui.painter().add(egui::Shape::Noop);
            let mut routed = false;
            let mut y = area.top();
            for group in hidden {
                let (h, r) =
                    pick_group(ui, group, egui::pos2(area.left(), y), DOCK_W, font, &mut state.active);
                y += h;
                routed |= r;
            }
            let panel = area.expand(Style::SP_S);
            ui.painter()
                .set(bg, egui::Shape::rect_filled(panel, Style::RADIUS, Style::SURFACE));
            ui.painter().rect_stroke(
                panel,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            routed
        });

    let routed = inner.inner;
    if routed {
        // A route closes the popup (the tray idiom).
        state.overflow_open = false;
    } else if !opened && inner.response.clicked_elsewhere() {
        // Click-away dismissal — but not on the very click that opened it (that
        // click lands outside the popup and would dismiss it in the same frame).
        state.overflow_open = false;
    }
    routed
}

#[cfg(test)]
mod tests {
    use super::{
        cell_id, dock, group_hairline_id, group_height, group_label_id, icon_texture,
        overflow_more_id, pick_cell_id, taskbar, underline, visible_group_count, DockState, Surface,
        CELL_W, DOCK_AREA, DOCK_W, GROUPS, GROUP_GAP, HAIRLINE_W, ICON_GAP, ICON_LOGICAL, LABEL_PAD,
        PIN_STRIP_H, SHOW_DESKTOP_W, TASKBAR_H, TASKBAR_TOP_PAD,
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

    /// Mount the real bottom bar (with a default tray over an unseen mesh) for one
    /// headless frame at a given screen `width` and return the frame output — the
    /// same `Context::run` → `TopBottomPanel::bottom` path `main.rs` mounts (matching
    /// its exact-height + zero-margin `SURFACE` frame), so the layout the harness
    /// measures is the live one.
    fn run_taskbar_sized(
        ctx: &egui::Context,
        active: &mut Surface,
        events: Vec<egui::Event>,
        width: f32,
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
                egui::vec2(width, 600.0),
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

    /// Mount the real bottom bar at the default 1280-wide screen (the click/glyph
    /// tests' width) for one headless frame.
    fn run_taskbar(
        ctx: &egui::Context,
        active: &mut Surface,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        run_taskbar_sized(ctx, active, events, 1280.0)
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

    // ── PICKER-3: the headless taskbar LAYOUT HARNESS ─────────────────────────
    // A repeatable, headless measurement of the grouped taskbar's REAL on-screen
    // geometry. It mounts the true `taskbar()` via `egui::Context::run` at a given
    // screen width, reads every element's settled `Rect` back by the stable id the
    // dock assigns (icon/Settings/Desktop cells → `cell_id`; rotated group labels →
    // `group_label_id`; Carbon-blue hairlines → `group_hairline_id`), and reduces
    // them to the group spacing rhythm. `report()` prints the measured geometry
    // (visible under `--nocapture`); `assert_even_rhythm()` pins the intended
    // rhythm as the regression guard + the spec.

    /// The measured geometry of one group in the app row.
    struct GroupGeom {
        label: &'static str,
        label_rect: egui::Rect,
        hairline_rect: egui::Rect,
        icons: Vec<egui::Rect>,
    }

    impl GroupGeom {
        fn first_icon(&self) -> egui::Rect {
            *self.icons.first().expect("a group has ≥1 icon")
        }
        fn last_icon(&self) -> egui::Rect {
            *self.icons.last().expect("a group has ≥1 icon")
        }
        /// The group's full horizontal extent — its rotated label is the leftmost
        /// element, its last icon the rightmost.
        fn left(&self) -> f32 {
            self.label_rect.left()
        }
        fn right(&self) -> f32 {
            self.last_icon().right()
        }
        /// label → Carbon-blue hairline gap.
        fn label_to_hairline(&self) -> f32 {
            self.hairline_rect.left() - self.label_rect.right()
        }
        /// hairline → first icon gap.
        fn hairline_to_first_icon(&self) -> f32 {
            self.first_icon().left() - self.hairline_rect.right()
        }
        /// The gap between consecutive icon cells within the group (`None` for a
        /// single-icon group).
        fn icon_to_icon(&self) -> Option<f32> {
            (self.icons.len() > 1).then(|| self.icons[1].left() - self.icons[0].right())
        }
    }

    /// The measured geometry of the whole taskbar at one screen width.
    struct BarGeom {
        width: f32,
        bar_top: f32,
        bar_bottom: f32,
        workbench: egui::Rect,
        groups: Vec<GroupGeom>,
        settings: egui::Rect,
        desktop: egui::Rect,
    }

    impl BarGeom {
        fn bar_center_y(&self) -> f32 {
            f32::midpoint(self.bar_top, self.bar_bottom)
        }
        /// The element immediately to the LEFT of group `i` — the Workbench lead for
        /// the first group, else the previous group's last icon.
        fn left_neighbour_right(&self, i: usize) -> f32 {
            if i == 0 {
                self.workbench.right()
            } else {
                self.groups[i - 1].right()
            }
        }
        /// The inter-group gap: the clear space BEFORE group `i`'s rotated label
        /// (measured off its left neighbour's right edge).
        fn pre_label_gap(&self, i: usize) -> f32 {
            self.groups[i].label_rect.left() - self.left_neighbour_right(i)
        }
        /// The flexible gap between the grouped run and the right cluster — the
        /// Settings button is the leftmost element of that cluster.
        fn group_to_tray_gap(&self) -> f32 {
            self.settings.left() - self.groups.last().expect("six groups").right()
        }

        /// Emit the full measured geometry as a table — every element's rect + the
        /// per-group gaps (deliverable #3). Printed by the harness test under
        /// `--nocapture`; also the human-readable form of what the assertions pin.
        fn report(&self) -> String {
            use std::fmt::Write as _;
            let mut s = String::new();
            let _ = writeln!(
                s,
                "=== taskbar layout @ {:.0}px  (bar y=[{:.1},{:.1}] center={:.1}) ===",
                self.width,
                self.bar_top,
                self.bar_bottom,
                self.bar_center_y()
            );
            let _ = writeln!(
                s,
                "lead  Workbench      x=[{:.1},{:.1}]",
                self.workbench.left(),
                self.workbench.right()
            );
            for (i, g) in self.groups.iter().enumerate() {
                let i2i = g
                    .icon_to_icon()
                    .map_or_else(|| "n/a".to_owned(), |v| format!("{v:.1}"));
                let _ = writeln!(
                    s,
                    "grp{i} {:<10} label x=[{:.1},{:.1}] cy={:.1} | hairline x={:.1} | \
icons x=[{:.1}..{:.1}] | grp=[{:.1},{:.1}] || pre={:.1} lbl→hr={:.1} hr→ic={:.1} ic→ic={}",
                    g.label,
                    g.label_rect.left(),
                    g.label_rect.right(),
                    g.label_rect.center().y,
                    g.hairline_rect.center().x,
                    g.first_icon().left(),
                    g.last_icon().right(),
                    g.left(),
                    g.right(),
                    self.pre_label_gap(i),
                    g.label_to_hairline(),
                    g.hairline_to_first_icon(),
                    i2i,
                );
            }
            let _ = writeln!(
                s,
                "right  group→tray gap={:.1} | Settings x=[{:.1},{:.1}] | tray x=[{:.1},{:.1}] | \
Desktop x=[{:.1},{:.1}]",
                self.group_to_tray_gap(),
                self.settings.left(),
                self.settings.right(),
                self.settings.right(),
                self.desktop.left(),
                self.desktop.left(),
                self.desktop.right(),
            );
            s
        }
    }

    /// Mount the real taskbar headlessly at `width`, settle the layout, and read
    /// every element's settled `Rect` back by its stable id — the harness core.
    fn measure_taskbar(width: f32) -> BarGeom {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::Workbench;
        // Prime two frames so every stable-id widget rect is registered + settled
        // (`read_response` reads the previous frame's rects — the W10-2 idiom).
        let _ = run_taskbar_sized(&ctx, &mut active, Vec::new(), width);
        let _ = run_taskbar_sized(&ctx, &mut active, Vec::new(), width);

        let rect_of = |id: egui::Id| {
            ctx.read_response(id)
                .expect("every taskbar element rect is registered under its stable id")
                .rect
        };
        let workbench = rect_of(cell_id(Surface::Workbench));
        let settings = rect_of(cell_id(Surface::System));
        let desktop = rect_of(cell_id(Surface::Desktop));
        let groups = GROUPS
            .iter()
            .map(|g| GroupGeom {
                label: g.label,
                label_rect: rect_of(group_label_id(g.label)),
                hairline_rect: rect_of(group_hairline_id(g.label)),
                icons: g.surfaces.iter().map(|&s| rect_of(cell_id(s))).collect(),
            })
            .collect();
        BarGeom {
            width,
            bar_top: workbench.top(),
            bar_bottom: workbench.bottom(),
            workbench,
            groups,
            settings,
            desktop,
        }
    }

    /// The realistic screen widths the harness pins the rhythm at — the T470s
    /// panel + two common desktop sizes.
    const HARNESS_WIDTHS: [f32; 3] = [1366.0, 1920.0, 2560.0];

    /// Layout tolerance in logical px — the gaps are added from exact `Style`
    /// tokens, so this only absorbs egui's sub-pixel rounding of the rects.
    const LAYOUT_TOL: f32 = 1.0;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= LAYOUT_TOL
    }

    /// Assert the intended even rhythm on a measured bar (the spec + regression
    /// guard). Every gap must equal its `Style` token and be equal across groups,
    /// the labels vertically centred, the hairlines aligned, and nothing overlapping.
    fn assert_even_rhythm(bar: &BarGeom) {
        let w = bar.width;
        assert_eq!(bar.groups.len(), 6, "@{w}: six measured groups");

        // (1) Inter-group gaps equal (within 1px) — the clear space before every
        // rotated label, incl. the Workbench-lead → first-group gap, is GROUP_GAP.
        for i in 0..bar.groups.len() {
            let gap = bar.pre_label_gap(i);
            assert!(
                approx(gap, GROUP_GAP),
                "@{w}: group {} pre-label gap {gap:.2} ≠ GROUP_GAP {GROUP_GAP}",
                bar.groups[i].label
            );
        }

        // (2) label→hairline→icon spacing consistent across groups — both pads are
        // LABEL_PAD and identical group to group (the rhythm the operator flagged).
        for g in &bar.groups {
            assert!(
                approx(g.label_to_hairline(), LABEL_PAD),
                "@{w}: {} label→hairline {:.2} ≠ LABEL_PAD {LABEL_PAD}",
                g.label,
                g.label_to_hairline()
            );
            assert!(
                approx(g.hairline_to_first_icon(), LABEL_PAD),
                "@{w}: {} hairline→icon {:.2} ≠ LABEL_PAD {LABEL_PAD}",
                g.label,
                g.hairline_to_first_icon()
            );
            // Icon-to-icon within a multi-icon cluster is the tight ICON_GAP.
            if let Some(i2i) = g.icon_to_icon() {
                assert!(
                    approx(i2i, ICON_GAP),
                    "@{w}: {} icon→icon {i2i:.2} ≠ ICON_GAP {ICON_GAP}",
                    g.label
                );
            }
        }

        // (3) Labels vertically centred in the bar.
        for g in &bar.groups {
            assert!(
                approx(g.label_rect.center().y, bar.bar_center_y()),
                "@{w}: {} label cy {:.2} not centred in the bar (center {:.2})",
                g.label,
                g.label_rect.center().y,
                bar.bar_center_y()
            );
        }

        // (4) Hairlines aligned — same 1px width + the same inset vertical extent
        // across every group (a clean shared rule, not a ragged set).
        let h0 = bar.groups[0].hairline_rect;
        for g in &bar.groups {
            assert!(
                approx(g.hairline_rect.width(), HAIRLINE_W),
                "@{w}: {} hairline width {:.2} ≠ HAIRLINE_W {HAIRLINE_W}",
                g.label,
                g.hairline_rect.width()
            );
            assert!(
                approx(g.hairline_rect.top(), h0.top())
                    && approx(g.hairline_rect.bottom(), h0.bottom()),
                "@{w}: {} hairline y-extent [{:.2},{:.2}] ≠ [{:.2},{:.2}]",
                g.label,
                g.hairline_rect.top(),
                g.hairline_rect.bottom(),
                h0.top(),
                h0.bottom()
            );
        }

        // (5) No overlap — each group's label starts strictly right of its left
        // neighbour's icons (the GROUP_GAP always separates them).
        for i in 0..bar.groups.len() {
            assert!(
                bar.groups[i].label_rect.left() > bar.left_neighbour_right(i),
                "@{w}: {} label overlaps the element to its left",
                bar.groups[i].label
            );
        }

        // (6) The right cluster keeps its positions with an even, positive flexible
        // gap: Settings sits left of the tray + the far-right Desktop sliver, and
        // the Desktop sliver still hugs the bar's right edge.
        assert!(
            bar.group_to_tray_gap() > 0.0,
            "@{w}: the grouped run collided with the right cluster (gap {:.2})",
            bar.group_to_tray_gap()
        );
        assert!(
            bar.settings.right() <= bar.desktop.left() + LAYOUT_TOL,
            "@{w}: Settings is not left of the Desktop sliver",
        );
        assert!(
            approx(bar.desktop.right(), w),
            "@{w}: the Desktop sliver no longer hugs the right edge (right {:.2})",
            bar.desktop.right()
        );

        // (7) The MEASURED gaps form a clear visual hierarchy — a group reads as
        // one cluster set clearly apart from the next: pre-label ≫ label-pad >
        // icon gap (checked on the rendered numbers, not just the token defs).
        let pre = bar.pre_label_gap(0);
        let pad = bar.groups[0].label_to_hairline();
        let icon = bar.groups[0]
            .icon_to_icon()
            .expect("the Comms group has two icons");
        assert!(
            pre > pad && pad > icon,
            "@{w}: gaps not tiered — pre-label {pre:.1} > label-pad {pad:.1} > icon {icon:.1}"
        );
    }

    #[test]
    fn the_grouped_taskbar_is_evenly_spaced_at_common_widths() {
        // PICKER-3 — the layout harness: measure the real taskbar geometry at the
        // T470s + two common desktop widths, print the report (seen under
        // `--nocapture`), and assert the even rhythm. This is both the spec and the
        // regression guard for the group spacing.
        for width in HARNESS_WIDTHS {
            let bar = measure_taskbar(width);
            // Emitted geometry — visible when the suite runs with `--nocapture`.
            eprint!("{}", bar.report());
            assert_even_rhythm(&bar);
        }
    }

    // ── VDOCK-1: the left vertical dock frame + auto-hide ─────────────────────

    /// Drive ONE headless frame of the vertical dock over a stand-in surface at a
    /// given screen `size`, feeding `events` — the routing/overflow harness core
    /// (the same `Context::run` path the DRM runner drives, minus the GPU).
    fn drive_vdock(
        ctx: &egui::Context,
        state: &mut DockState,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            // A stand-in surface beneath the dock (the background layer).
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            let _ = dock(ctx, state);
        });
    }

    /// Drive `frames` quiet headless frames of the vertical dock on a 1280×800
    /// screen (the VDOCK-1 passthrough/frame tests' size).
    fn run_vdock(ctx: &egui::Context, state: &mut DockState, frames: usize) {
        for _ in 0..frames {
            drive_vdock(ctx, state, Vec::new(), egui::vec2(1280.0, 800.0));
        }
    }

    /// The dock's floating-Area `LayerId` — `LayerId::new(Foreground, DOCK_AREA)`,
    /// the same mapping `egui::Area::layer()` computes.
    fn vdock_layer() -> egui::LayerId {
        egui::LayerId::new(egui::Order::Foreground, egui::Id::new(DOCK_AREA))
    }

    #[test]
    fn the_vertical_dock_is_a_48px_full_height_column() {
        // Locks #2/#23 — the dock is one 48px-wide column, sharing the horizontal
        // taskbar's 48px icon-cell module (so VDOCK-2/3/4 inherit the grid).
        assert!((DOCK_W - 48.0).abs() < f32::EPSILON, "dock width ~48px");
        assert!(
            (DOCK_W - CELL_W).abs() < f32::EPSILON,
            "dock shares the taskbar cell module"
        );
    }

    #[test]
    fn the_dock_state_super_toggle_and_pin_hold_it_open() {
        // Locks #9/#13 — the pure auto-hide state machine (no GPU): the dock is
        // hidden by default, a Super tap toggles the reveal, and the pin holds it
        // open regardless of the reveal latch.
        let mut s = DockState::default();
        assert!(!s.shown(), "hidden by default (lock #9)");

        s.toggle();
        assert!(s.shown(), "a Super tap reveals it (lock #13)");
        s.toggle();
        assert!(!s.shown(), "a second tap hides it");

        // Pin holds it open even when the reveal latch is off.
        s.toggle_pin();
        assert!(
            s.pinned() && s.shown(),
            "pinning shows + holds it (lock #9)"
        );
        s.toggle();
        assert!(
            s.shown(),
            "a Super tap can't hide a PINNED dock — the pin holds it open"
        );
        // Unpinning (with the reveal latch now off) lets it hide again.
        s.toggle_pin();
        assert!(!s.shown(), "unpinning releases the hold");
    }

    #[test]
    fn a_hidden_dock_mounts_no_layer_so_input_passes_through() {
        // The design's "auto-hide + DRM seat" risk: while hidden the dock must not
        // float a layer over the surface, or it would steal clicks/keys meant for
        // the surface beneath. A hidden dock creates NO Area, so `layer_id_at` over
        // its would-be column finds no dock layer — the click reaches the surface.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut hidden = DockState::default(); // hidden by default
        run_vdock(&ctx, &mut hidden, 2);

        let point = egui::pos2(DOCK_W / 2.0, 400.0); // inside the would-be column
        assert_ne!(
            ctx.layer_id_at(point),
            Some(vdock_layer()),
            "a HIDDEN dock must not float an intercepting layer (input passthrough)"
        );
    }

    #[test]
    fn a_shown_dock_covers_its_column_and_paints_the_carbon_panel() {
        // The mirror of the passthrough test: a shown dock DOES claim its column
        // (so clicks over it land on the dock, not the surface), and its frame draws
        // real primitives (the Carbon-dark fill + the right-edge divider).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut shown = DockState::default();
        shown.toggle(); // reveal it
        assert!(shown.shown());

        // Prime one frame, then capture the second frame's output.
        run_vdock(&ctx, &mut shown, 1);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            let _ = dock(ctx, &mut shown);
        });

        let point = egui::pos2(DOCK_W / 2.0, 400.0);
        assert_eq!(
            ctx.layer_id_at(point),
            Some(vdock_layer()),
            "a SHOWN dock claims its column so clicks land on the dock chrome"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the shown dock frame painted nothing");
    }

    #[test]
    fn clicking_the_pin_toggle_pins_the_dock_open() {
        // The pin affordance (lock #9) is reachable: a click in the top cell flips
        // the pin, holding the dock open. Mirrors the taskbar cell-click test —
        // prime the layout, then press one frame + release the next.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal it so the Area (and its pin) is mounted

        // VDOCK-2 folded the pin into the slim strip just BENEATH the Workbench
        // lead cell (the top DOCK_W-tall cell is now the Workbench); click the pin
        // strip's centre.
        let click = egui::pos2(DOCK_W / 2.0, DOCK_W + PIN_STRIP_H / 2.0);
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
        let frame = |ctx: &egui::Context, s: &mut DockState, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1280.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = ui.button("surface");
                });
                let _ = dock(ctx, s);
            });
        };
        // Prime two frames so egui has the pin's rect registered (and the Area is
        // past its first-show sizing pass), then move onto the pin + press, then
        // release the next frame — the egui click model the taskbar test uses.
        frame(&ctx, &mut s, Vec::new());
        frame(&ctx, &mut s, Vec::new());
        frame(&ctx, &mut s, vec![egui::Event::PointerMoved(click), press]);
        frame(&ctx, &mut s, vec![release]);
        assert!(s.pinned(), "clicking the pin holds the dock open (lock #9)");
    }

    // ── VDOCK-2: the vertical app picker (top + middle zones) ─────────────────

    /// The picker's surfaces in order — the Workbench lead, then each group's
    /// members (`Surface::ALL` order). Excludes System (Settings) + Desktop, which
    /// are VDOCK-4's system quad.
    fn picker_surfaces() -> Vec<Surface> {
        std::iter::once(Surface::Workbench)
            .chain(GROUPS.iter().flat_map(|g| g.surfaces.iter().copied()))
            .collect()
    }

    fn press_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn release_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Click `center` — press one frame, release the next (the egui click model
    /// the taskbar tests use). The caller primes the layout first.
    fn click_vdock(ctx: &egui::Context, state: &mut DockState, center: egui::Pos2, size: egui::Vec2) {
        drive_vdock(
            ctx,
            state,
            vec![egui::Event::PointerMoved(center), press_at(center)],
            size,
        );
        drive_vdock(ctx, state, vec![release_at(center)], size);
    }

    #[test]
    fn the_app_zone_fits_all_groups_when_tall_and_overflows_when_short() {
        // #22 — all six groups render inline when the app zone is tall enough; a
        // short zone reserves the '…' cell and shows fewer WHOLE groups.
        let total: f32 = GROUPS.iter().map(group_height).sum();
        assert_eq!(
            visible_group_count(total),
            GROUPS.len(),
            "all six fit when the zone == their total height"
        );
        assert_eq!(
            visible_group_count(total + 100.0),
            GROUPS.len(),
            "all six fit with room to spare"
        );
        // Drop just under the total (by the last group's height) → at least one
        // group folds into the overflow popup.
        let short = total - group_height(&GROUPS[GROUPS.len() - 1]);
        let n = visible_group_count(short);
        assert!(
            n < GROUPS.len(),
            "a short zone overflows — showed {n} of {}",
            GROUPS.len()
        );
        // A zone too small for even one group shows none (everything overflows).
        assert_eq!(visible_group_count(0.0), 0, "no room → all overflow");
    }

    #[test]
    fn the_picker_routes_every_app_surface_and_defers_the_system_quad() {
        // §7 — the Workbench lead + the twelve group surfaces each route on a click
        // into DockState::active (the carried-over routing). Settings (System) +
        // Show-Desktop are NOT in the picker — they belong to VDOCK-4's system quad.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal the dock so its Area (and cells) mount
        let sz = egui::vec2(1280.0, 800.0);
        // Prime so every stable-id cell rect is registered + settled.
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let picker = picker_surfaces();
        assert_eq!(
            picker.len(),
            Surface::ALL.len() - 2,
            "the picker holds every surface but System + Desktop"
        );

        // Read every picker cell's settled centre up front (a click shifts no rect).
        let mut centers: Vec<(Surface, egui::Pos2)> = Vec::new();
        for &want in &picker {
            let resp = ctx.read_response(pick_cell_id(want));
            assert!(resp.is_some(), "{want:?} picker cell rect not registered");
            centers.push((want, resp.expect("registered above").rect.center()));
        }

        for (want, center) in centers {
            click_vdock(&ctx, &mut s, center, sz);
            assert_eq!(s.active, want, "clicking {want:?}'s picker cell selects it");
        }

        // The system-quad surfaces are absent from the picker (VDOCK-4 owns them).
        assert!(
            ctx.read_response(pick_cell_id(Surface::System)).is_none(),
            "System (Settings) is deferred to VDOCK-4's system quad"
        );
        assert!(
            ctx.read_response(pick_cell_id(Surface::Desktop)).is_none(),
            "Show-Desktop is deferred to VDOCK-4's system quad"
        );
    }

    #[test]
    fn the_picker_stacks_the_groups_in_a_single_column() {
        // #2 — the app picker is ONE vertical column: every cell shares the
        // column's x-centre + full width, and the cells march strictly downward.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let mut prev_bottom = f32::MIN;
        for surface in picker_surfaces() {
            let resp = ctx.read_response(pick_cell_id(surface));
            assert!(resp.is_some(), "{surface:?} cell rect not registered");
            let rect = resp.expect("registered above").rect;
            assert!(
                (rect.center().x - DOCK_W / 2.0).abs() < 1.0,
                "{surface:?} cell off the column centre (cx {})",
                rect.center().x
            );
            assert!(
                (rect.width() - DOCK_W).abs() < 1.0,
                "{surface:?} cell is not the full column width"
            );
            assert!(
                rect.top() >= prev_bottom - 1.0,
                "{surface:?} cell is not stacked below the previous one"
            );
            prev_bottom = rect.bottom();
        }
    }

    #[test]
    fn the_group_labels_paint_horizontally_in_their_group_accent() {
        // #4 — each group carries ONE horizontal (angle 0) accent label above its
        // cells, painted in that group's Style accent token; on a tall screen all
        // six render inline with no '…' overflow.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        // Prime a frame, then capture over an EMPTY surface so the only text is the
        // dock's group labels (no stand-in button caption to filter out).
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), sz)),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |_ui| {});
            let _ = dock(ctx, &mut s);
        });
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_text_shapes(&clipped.shape, &mut texts);
        }
        assert_eq!(
            texts.len(),
            GROUPS.len(),
            "exactly one label per group, nothing else (no captions, no '…' at this height)"
        );
        let accents: Vec<egui::Color32> = GROUPS.iter().map(|g| g.accent).collect();
        for (angle, color) in texts {
            assert!(
                angle.abs() < 1e-3,
                "the vertical dock's labels read HORIZONTALLY (angle 0), got {angle}"
            );
            assert!(
                accents.contains(&color),
                "a group label is painted in its group accent, got {color:?}"
            );
        }
    }

    #[test]
    fn the_active_surface_wears_a_left_edge_accent_bar() {
        // #10 — the active cell wears a left-edge Style::ACCENT bar. Capture the
        // frame's rect_filled shapes and confirm an ACCENT-coloured rect hugs the
        // column's left edge (x≈0) at the active cell — absent for the inactive.
        fn left_edge_accent_bars(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(r) if r.fill == Style::ACCENT && r.rect.left() < 1.0 => {
                    out.push(r.rect);
                }
                egui::Shape::Vec(v) => {
                    for s in v {
                        left_edge_accent_bars(s, out);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default(); // active = Workbench (the top lead cell)
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), sz)),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |_ui| {});
            let _ = dock(ctx, &mut s);
        });

        let mut bars = Vec::new();
        for clipped in &out.shapes {
            left_edge_accent_bars(&clipped.shape, &mut bars);
        }
        // Exactly the active (Workbench) lead cell shows a left-edge accent bar.
        assert_eq!(
            bars.len(),
            1,
            "one active left-edge accent bar (the Workbench lead), got {}",
            bars.len()
        );
        let wb = ctx
            .read_response(pick_cell_id(Surface::Workbench))
            .expect("the Workbench lead cell is registered")
            .rect;
        let bar = bars[0];
        assert!(bar.left() < 1.0, "the accent bar hugs the column's left edge");
        assert!(
            (bar.height() - wb.height()).abs() < 1.0,
            "the bar spans the active cell's height"
        );
    }

    #[test]
    fn the_overflow_more_popup_routes_a_hidden_group_surface() {
        // #22 — on a short screen the lower groups fold into the '…' more-popup:
        // the '…' cell is present, clicking it opens the popup, and a popup cell
        // still routes to its Surface (then closes the popup).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        // Short enough that the last groups (incl. Media) overflow the app zone.
        let sz = egui::vec2(1280.0, 600.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let more = ctx
            .read_response(overflow_more_id())
            .expect("the '…' overflow cell is registered on a short screen")
            .rect;
        assert!(!s.overflow_open, "the popup starts closed");
        assert!(
            ctx.read_response(pick_cell_id(Surface::Media)).is_none(),
            "Media is folded into the overflow, not an inline cell yet"
        );

        // Click '…' → the popup opens.
        click_vdock(&ctx, &mut s, more.center(), sz);
        assert!(s.overflow_open, "clicking '…' opens the more-popup");

        // Settle the popup so its cells register, then click Media inside it.
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let media = ctx
            .read_response(pick_cell_id(Surface::Media))
            .expect("the overflowed Media cell renders in the popup")
            .rect
            .center();
        click_vdock(&ctx, &mut s, media, sz);
        assert_eq!(
            s.active,
            Surface::Media,
            "a click in the more-popup routes to its Surface"
        );
        assert!(!s.overflow_open, "routing from the popup closes it");
    }
}
