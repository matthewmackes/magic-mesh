//! The shell **dock** — the left **vertical dock** ([`dock`], design
//! `docs/design/vertical-dock.md`) plus the full-width **bottom taskbar**
//! ([`notification_rail_with_sources`], design `docs/design/
//! win7-desktop-survey.md`, WIN7-1): together the shell's ONE chrome (VDOCK, the
//! sole surface launcher after VDOCK-6b ripped out the old horizontal taskbar;
//! NAVBAR/NOTIF/CONSOLE-1 then built the *new* bottom rail that WIN7-1 formalizes
//! as a true Win7-style taskbar).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The dock
//! is that shell nav: a left-edge, full-height, ~48px slide-in auto-hide column
//! that selects which surface fills the shell body — the mesh-control
//! [`Workbench`](Surface::Workbench), the live Mesh Map, the Desktop surface, the
//! embedded app surfaces (Music / Media / Files /
//! Voice / Browser / Terminal / Editor), the unified [`Chat`](Surface::Chat)
//! surface, and the System / Storage / About screens. One surface shows at a
//! time; the Workbench is always one click away.
//!
//! The picker leads with the **Workbench** as the top standalone anchor, then the
//! surfaces gathered into six labelled **groups** (PICKER-1: Comms · Workloads ·
//! Terminals · Mesh · System · Media) stacked single-column, each with a
//! horizontal accent label + a left-rail accent stripe over its 24px brand glyph
//! cells (in [`Surface::ALL`] order). The active cell wears a **left-edge accent
//! bar** + the subtle selection wash; hover is a fill only — no per-icon captions,
//! no tooltips anywhere. Beneath the picker sits only VDOCK-4's **system quad**
//! (Settings · Show-Desktop · Lock · Power) — the Start cell, the session rail,
//! the clock, the auto-hide pin, and the notification status pips all live in the
//! separate full-width **bottom taskbar** (WIN7-1 lock #3's Start · sessions ·
//! tray · clock order, with the pin trailing after the clock), never in this left
//! rail.
//!
//! The dock is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.
//!
//! The dock slides in from the left over the shared [`Motion`] table and
//! auto-hides (Super-tap toggle + pin, no hover). When fully hidden + settled it
//! mounts **no layer at all**, so a hidden dock steals no input from the surface
//! beneath (the "auto-hide + DRM seat" guarantee).

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::{Density, GradeBand, Motion, Style};
use mde_seat::{PowerVerb, SeatSnapshot};
use mde_theme::brand::icons::{icon_image, IconId};

use crate::chrome::{GradeRow, MeshSummary, NodeGrades};
use crate::status::{self, StatusSegments};

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
    /// The **Explorer** — the EXPLORER-epic discovery surface (`crate::explorer`):
    /// a cinematic one-unit-at-a-time hero view over every discovered unit (mesh
    /// peers · off-mesh LAN hosts · `OpenStack` objects), folded from the
    /// aggregator's `state/units/*` mirrors. A first-class dock surface (its Mesh
    /// sibling beside the Mesh Map); it is ALSO reachable as the Mesh Map's
    /// segmented Explorer lens, which powers the NODE-GRADE-2 node-focus jump.
    Explorer,
    /// The VDI **Desktop** surface — a brokered VM desktop rendered egui-native
    /// (`mde-vdi-rdp` / `mde-vdi-vnc`), the point of E12 "Quasar".
    Desktop,
    /// The **Infra as Code (`IaC`)** surface — the `OpenStack` `IaaS` control
    /// plane (`docs/design/iac-workspace.md`, IAC-2): the Keystone service
    /// catalog + per-service API health + the merged service directory, consumed
    /// off the Bus (`action/cloud/get-catalog`). The comprehensive `OpenStack`
    /// admin beside the member-facing Cloud plane (#24).
    InfraCode,
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
    /// The embedded Bookmarks manager (`mde-bookmarks-egui`) — folders, tags,
    /// search, import, and bookmark detail management over the mesh CRDT model.
    Bookmarks,
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
    /// The **Phones** hub surface (KDC-MESH-9) — the desktop-side management surface
    /// for the mesh's paired phone(s): mesh identity + battery/signal, per-feature
    /// toggles, the node-targeted file browser, the run-command catalog (incl. the
    /// OpenStack lifecycle set), fast mesh-wide unpair, and the pair-a-phone flow. A
    /// thin client of the `kdc_host` worker (the `action/connect/*` verbs + the mesh
    /// service directory) — it renders published state + drives Bus verbs, never
    /// reimplementing the host (§6).
    Phones,
    /// The System surface — this seat's host controls (audio mixer, Bluetooth,
    /// displays, power & battery, backlight, hotkeys), folded from `mde-seat`
    /// (E12-15). Owns ALL host-control interaction (lock 3); dock status keeps
    /// only read-only summaries.
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
    /// The **Timers & Alarms** surface (VDOCK-5, locks #5/#16/#20) — the clock's
    /// replacement: countdown timers + daily alarms whose firings ride the
    /// CHAT-FIX-2 `event/notify/timer` lane (`crate::timers`). Deliberately NOT
    /// in [`Surface::ALL`]/the picker: its one home is the dock's clock-glyph
    /// cell ([`clock_cell`] — the live time IS the glyph, lock #20).
    Timers,
}

// This nav enum spells its variants `Surface::Music` rather than `Self::Music` on
// purpose: the explicit type keeps the `ALL` table and the glyph map scannable
// side by side (a launcher reads clearer than a wall of `Self::`). Opt the block
// out of `clippy::use_self` rather than thread `Self::` through every arm.
#[allow(clippy::use_self)]
impl Surface {
    /// Every surface in canonical order — the ordering authority the picker is
    /// built + checked against: the Workbench (mesh-control home) first, then the
    /// live Mesh Map, the Cloud/IaC control surface + the brokered Desktop, the
    /// app surfaces, the unified Chat surface (the ONE notification interface),
    /// and the System / Storage / About screens. PICKER-1 gathers these into the
    /// labelled [`GROUPS`] (the Workbench leads standalone), preserving this
    /// relative order within each group (L7); a compile-time guard keeps the two
    /// tables in sync.
    pub(crate) const ALL: [Surface; 18] = [
        Surface::Workbench,
        Surface::MeshView,
        Surface::Explorer,
        Surface::InfraCode,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Voice,
        Surface::Browser,
        Surface::Bookmarks,
        Surface::Terminal,
        Surface::Editor,
        Surface::Chat,
        Surface::Phones,
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
            // The Explorer wears the stacked-cards **Instances** glyph — the
            // "a deck of discovered units you page through" reading fits the
            // hero-card filmstrip, and it stays distinct from the Mesh Map's
            // topology glyph beside it in the Mesh group.
            Surface::Explorer => IconId::Instances,
            // The IaC surface wears the **Server** (infrastructure/rack) badge —
            // the OpenStack IaaS control plane reads as "infrastructure".
            Surface::InfraCode => IconId::Server,
            Surface::Desktop => IconId::Desktop,
            Surface::Music => IconId::Music,
            Surface::Media => IconId::Media,
            Surface::Files => IconId::Files,
            Surface::Voice => IconId::Voice,
            Surface::Browser => IconId::Browser,
            Surface::Bookmarks => IconId::Bookmarks,
            Surface::Terminal => IconId::Terminal,
            Surface::Editor => IconId::Editor,
            Surface::Chat => IconId::Chat,
            // The Phones hub wears the dedicated smartphone glyph (KDC-MESH-9).
            Surface::Phones => IconId::Phones,
            // The System (host-controls) surface is the dock's right-side Settings
            // button (PICKER-2) — it wears the toothed **cog** glyph, the Win10
            // settings-gear idiom, distinct from the spoked legacy System glyph.
            Surface::System => IconId::Settings,
            Surface::Storage => IconId::Storage,
            // The About surface wears the product **mark** — the mesh-node
            // constellation glyph that IS the platform identity — fitting the
            // "about this platform" screen and distinct from every surface glyph.
            // Timers shares the arm for exhaustiveness ONLY (lock #20): its dock
            // affordance is the LIVE TIME painted as text by [`clock_cell`]
            // ("shows the time as its glyph"), never a brand SVG, and it sits
            // outside `ALL`/the picker — no picker cell ever asks for this glyph.
            Surface::About | Surface::Timers => IconId::Mark,
        }
    }

    /// The tile/menu **display label** for this surface (WIN7-3, design
    /// `docs/design/win7-desktop-survey.md` lock #8's "icon + label" tile
    /// face). This is NOT an existing mapping the app picker already
    /// rendered: PICKER-1's own lock is "no per-icon captions, no tooltips
    /// anywhere" (this module's top doc comment), so unlike [`Self::icon_id`]
    /// there was no reusable label table for a tile grid to inherit — this
    /// method is new data, added alongside the icon map so a surface's two
    /// tile-facing facts live in one place. Human-facing names, matching each
    /// variant's own doc comment above (`MeshView` → "Mesh Map", `InfraCode`
    /// → "Infra as Code"; the rest a plain rendering of the variant name).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Surface::Workbench => "Workbench",
            Surface::MeshView => "Mesh Map",
            Surface::Explorer => "Explorer",
            Surface::InfraCode => "Infra as Code",
            Surface::Desktop => "Desktop",
            Surface::Music => "Music",
            Surface::Media => "Media",
            Surface::Files => "Files",
            Surface::Voice => "Voice",
            Surface::Browser => "Browser",
            Surface::Bookmarks => "Bookmarks",
            Surface::Terminal => "Terminal",
            Surface::Editor => "Editor",
            Surface::Chat => "Chat",
            Surface::Phones => "Phones",
            Surface::System => "System",
            Surface::Storage => "Storage",
            Surface::About => "About",
            Surface::Timers => "Timers & Alarms",
        }
    }
}

/// A shared bar-height token in logical points (`SP_XL + SP_M + SP_S` on the 8px
/// grid = 56px). The old horizontal taskbar mounted its bottom panel at exactly
/// this height; after VDOCK-6b removed that bar the token survives as the height
/// the boot backdrop reserves at the screen bottom (`backdrop.rs`) and the curtain
/// input-exclusivity test mounts its chrome strip at. (`pub`, not `pub(crate)`, is
/// the `clippy::redundant_pub_crate` form for a crate-visible item in a private
/// module.)
pub const TASKBAR_H: f32 = Style::SP_XL + Style::SP_M + Style::SP_S;

/// The fixed width of one icon-only glyph cell (lock W4 — no caption, so the
/// cell shrinks to suit the 24px glyph): `SP_XL + SP_M` on the 8px grid. The
/// vertical dock's [`DOCK_W`] column is this same module. Private: only the
/// dock's own layout + tests read it.
const CELL_W: f32 = Style::SP_XL + Style::SP_M;

/// The app glyph edge in logical points — the 24px dock icon (lock W3, `SP_L`).
/// Rasterized crisp at the physical pixel size by `icon_texture`.
const ICON_LOGICAL: f32 = Style::SP_L;

// ── PICKER-1: the app picker grouped into named sections ────────────────────

/// A named section of the app picker (PICKER-1): an accent label + the group's
/// 24px icon cells, keyed by the group's identity colour. The Workbench is NOT in
/// any group — it leads the picker as the top standalone anchor.
struct Group {
    /// The section heading, painted in [`Self::accent`].
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

/// The six labelled groups in their locked top-to-bottom order (L5), each
/// listing its surfaces in [`Surface::ALL`] relative order (L7). THREE surfaces
/// sit outside every group: the **Workbench** leads the picker as the top
/// standalone anchor, and **System** (Settings) + **Desktop** (Show-Desktop) are
/// VDOCK-4's bottom system-quad cells; every other surface appears here exactly
/// once (About lives in System's group) — the union with those three reproduces
/// all 18 of [`Surface::ALL`]. Drives the picker render + the shell tests (the one
/// grouping authority).
const GROUPS: [Group; 6] = [
    Group {
        label: "Comms",
        accent: Style::ACCENT_COMMS,
        surfaces: &[Surface::Voice, Surface::Chat, Surface::Phones],
    },
    Group {
        label: "Workloads",
        accent: Style::ACCENT_WORKLOADS,
        surfaces: &[Surface::InfraCode],
    },
    Group {
        label: "Terminals",
        accent: Style::ACCENT_TERMINALS,
        surfaces: &[
            Surface::Browser,
            Surface::Bookmarks,
            Surface::Terminal,
            Surface::Editor,
        ],
    },
    Group {
        label: "Mesh",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::MeshView, Surface::Explorer],
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

const PICKER_FOCUS_ORDER: [Surface; 17] = [
    Surface::Workbench,
    Surface::Voice,
    Surface::Chat,
    Surface::Phones,
    Surface::InfraCode,
    Surface::Browser,
    Surface::Bookmarks,
    Surface::Terminal,
    Surface::Editor,
    Surface::MeshView,
    Surface::Explorer,
    Surface::Files,
    Surface::Storage,
    Surface::About,
    Surface::Music,
    Surface::Media,
    Surface::Desktop,
];

// Compile-time guard: the Workbench lead + VDOCK-4's two system-quad cells
// (System/Settings + Desktop/Show-Desktop) + the six `GROUPS` place every
// `Surface::ALL` entry EXACTLY once — so the picker can never silently drop or
// duplicate a surface when either table changes (add a surface to `ALL` but forget
// to group it, or list it twice, and the crate fails to compile). Keeps
// `Surface::ALL` the authority the render is checked against. Fieldless enums cast
// to their discriminant in const, so this compares by identity.
const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        // Three surfaces are placed outside every group: Workbench (the top
        // standalone lead), System + Desktop (VDOCK-4's system-quad cells).
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
            "every Surface::ALL entry must be placed exactly once across the Workbench lead + the System + Desktop system-quad cells + GROUPS",
        );
        i += 1;
    }
};

/// The Carbon-blue group hairline width in logical points — a 1px rule (L3).
const HAIRLINE_W: f32 = 1.0;

/// The group-label point-size floor — the micro-label never shrinks below this,
/// so it stays legible even when a long label wants to overflow the narrow column.
const LABEL_MIN_PT: f32 = 8.0;

/// The shared point size for every group label — starts at [`Style::SMALL`] and
/// shrinks UNIFORMLY (all six labels together) just enough that the widest label
/// fits within `avail` logical points (the narrow dock column's interior width).
/// Floored at [`LABEL_MIN_PT`] for legibility.
fn group_label_font(ui: &egui::Ui, avail: f32) -> egui::FontId {
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
    let pt = if widest <= avail {
        Style::SMALL
    } else {
        (Style::SMALL * avail / widest).max(LABEL_MIN_PT)
    };
    egui::FontId::proportional(pt)
}

/// Rasterize + upload a brand glyph, cached in egui memory so a given
/// `(glyph, physical-size, tint)` triple is rasterized through `resvg` **once**
/// and then shared as a cheap ref-counted [`TextureHandle`] — never re-rasterized
/// per frame (the backdrop.rs lock-7 pattern). A failed rasterize caches `None`,
/// so a broken asset fails soft (§7) without retrying every frame. Shared by the
/// dock's status and system glyphs through the same cache.
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
// The shell's left-edge chrome (VDOCK-6b removed the old horizontal taskbar,
// then NAVBAR/NOTIF/CONSOLE-1 rebuilt a *new* bottom rail this module also owns —
// see [`notification_rail_with_sources`], WIN7-1's bottom taskbar): a left-edge,
// full-height, ~48px, solid Carbon-dark column that slides in from the left and
// auto-hides (hotkey + pin, no hover). VDOCK-1 builds the FRAME + the
// slide/toggle/pin mechanism; the interior is filled by the app picker (VDOCK-2)
// and the system quad (VDOCK-4). The Start cell, clock, auto-hide pin, and
// notification pips all mount in the separate bottom taskbar, not here.
// ═══════════════════════════════════════════════════════════════════════════

/// The vertical dock's width in logical points (~48px, design #2/#23) — one
/// column, the SAME 48px [`CELL_W`] module, so VDOCK-2's app glyphs + VDOCK-3/4's
/// quads inherit the grid. (`pub`, not `pub(crate)` — the
/// `clippy::redundant_pub_crate` form for a crate-visible item in a private
/// module, like [`TASKBAR_H`].)
pub const DOCK_W: f32 = CELL_W;

/// The egui memory key for the dock's slide animation (the Motion latch that
/// eases the reveal 0↔1). Private to the dock.
const DOCK_SLIDE_KEY: &str = "vdock-slide";

/// The egui memory key for NOTIF-4's right-side status detail panel.
const STATUS_PANEL_KEY: &str = "vdock-status-panel";

/// The stable id of the dock's floating [`egui::Area`] layer, so the shell (and
/// the passthrough test) can name its `LayerId` — `LayerId::new(Foreground,
/// Id::new(DOCK_AREA))`.
const DOCK_AREA: &str = "vdock-area";

/// The stable id of the bottom notification rail layer.
const NOTIFICATION_RAIL_AREA: &str = "notif-bottom-rail-area";

/// The left vertical dock's **state** — VDOCK-1's auto-hide inputs (locks #9/#13)
/// plus VDOCK-2's picker state. The auto-hide half (the Super-tap **reveal** latch
/// and the **pin**) is kept tiny and pure (no egui, no GPU) so the shell's hotkey
/// path toggles it and the render reads [`Self::shown`] headless-testably; there
/// is deliberately **no hover-reveal** (lock #9). VDOCK-2 adds the picker's
/// `active` surface (the shell body follows it, carried over from the horizontal
/// bar's routing) and the `overflow_open` popup latch (#22). The shell-side getter
/// [`Self::active`] reads `active` back into the central view (the VDOCK-6 `main.rs`
/// wire); [`Self::set_active`] mirrors the shell's live surface back in first, so a
/// hotkey / chyron nav that moved the surface still highlights in the picker.
// The dock carries several INDEPENDENT boolean latches (the reveal/pin auto-hide
// pair + the two overflow-popup latches for the app groups and the grade list) —
// not a state machine folding into one enum, so opt this one struct past the
// `struct_excessive_bools` bar rather than contrive a two-variant enum.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
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
    /// Whether the NODE-GRADE-2 grade list's '…' expander is open (design #8) — set
    /// by its '…' cell, cleared on a tap-route or a click-away. Distinct latch from
    /// the app picker's [`Self::overflow_open`].
    grades_overflow_open: bool,
    /// NOTIF-4 — whether the bottom notification rail's detail panel is open.
    /// Toggled by the rail chevron and dismissed by Esc or click-away.
    status_panel_open: bool,
    /// The live inputs NOTIF-3's bottom **notification rail** folds each frame —
    /// owned so `dock()` keeps its `(ctx, state)` signature; the shell refreshes it
    /// via [`Self::set_status_inputs`] before each `dock()`. Defaults to the honest
    /// pre-poll state.
    status: StatusInputs,
    /// VDOCK-4 — the system quad's **Power menu** (design #18): the anchored
    /// Lock/Suspend/Reboot/Shutdown popup off the Power cell, plus the typed-arming
    /// echo the two host-down verbs demand. Closed by default.
    power: PowerMenu,
    /// VDOCK-4 — a pending shell **request** the dock records for `main.rs` to
    /// drain after [`dock`] (the Lock/Power system-quad cells + the Power menu). The
    /// dock can't reach the shell's `Curtain`/seat, so it records the intent here
    /// and the shell drives it via [`Self::take_request`] — the deferred wire
    /// (VDOCK-3's `set_status_inputs` pattern), out of this dock.rs-only fence.
    /// `None` until a cell/menu fires (one request outlives one frame).
    pending: Option<DockRequest>,
    /// A pending **node-focus** request the NODE-GRADE-2 grade list records when a
    /// grade row is tapped (design #7): the hostname whose Explorer hero the shell
    /// should open. The dock can't reach the shell's Explorer / nav (§6), so it
    /// records the host here and `main.rs` drives the jump via
    /// [`Self::take_node_focus`] (the deferred-wire idiom, like [`Self::pending`]).
    /// A `String` (not `Copy`), so it rides its own field rather than [`DockRequest`].
    pending_node_focus: Option<String>,
    /// WIN7-2 — whether the Start Menu panel is up, mirrored in by the shell
    /// each frame ([`Self::set_start_menu_open`]) so the Start cell wears its
    /// active tint (the `set_active` mirror idiom). Was `console_open`
    /// pre-WIN7-2, when the Start cell opened Console directly.
    start_menu_open: bool,
    /// WIN7-2 — latched `true` by a Start-cell click ([`start_cell`]); the
    /// shell drains it ([`Self::take_start_menu_toggle`]) and toggles the
    /// Start Menu panel (`crate::start_menu`). The dock can't reach the panel
    /// itself (§6, the deferred wire). Was `console_toggle` pre-WIN7-2.
    start_menu_toggle: bool,
    /// NAVBAR-U1 — latched by the bottom-rail Desktop cell. The shell drains it
    /// and asks the chooser to reconnect the newest recent desktop, falling back
    /// to the chooser if no recent can connect.
    desktop_reconnect: bool,
    /// NAVBAR-U2 — whether the bottom-rail Desktop source flyout is open.
    desktop_sources_open: bool,
    /// NAVBAR-7 — whether the bottom rail's overflow More popup is open.
    rail_more_open: bool,
    /// NAVBAR-U2 — source id selected in the compact Desktop flyout. The shell
    /// drains it and hands it back to ChooserState's normal connect path.
    desktop_source_pick: Option<String>,
    /// NAVBAR-U3 — session id selected from the taskbar-style Desktop run. The
    /// shell drains it and focuses the Desktop face for that broker-visible
    /// session without inventing a second session store.
    desktop_session_focus: Option<String>,
    /// TRANSFERS-9 — the Files surface's in-flight transfer count, mirrored from
    /// the embedded Files ledger each frame. Zero paints no badge.
    transfer_active_count: usize,
    /// NAVBAR-8 — the shell-wide interaction density mirrored from the
    /// formfactor/control-surface path. Mouse keeps the compact icon rail; Touch
    /// expands the rail into the 48px labelled variant.
    density: Density,
    /// WIN10-HYBRID — the persisted taskbar **auto-hide** setting. Off by default:
    /// the bar stays docked and reserves its bottom strut. See
    /// [`set_taskbar_autohide`](Self::set_taskbar_autohide).
    taskbar_autohide: bool,
}

/// A shell-level **request** the VDOCK-4 system quad records for the shell to drain
/// after [`dock`] — the dock never reaches the `Curtain` or the seat itself (§6),
/// so it hands the intent back and `main.rs` drives the real seam. `Copy` (its
/// `PowerVerb` is), so recording one is a plain field assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockRequest {
    /// Drop the shell curtain — the Lock cell + the Power menu's Lock item. The
    /// shell drives `curtain.lock()` (the in-process lock, exactly like Super+L),
    /// NOT logind's session Lock.
    Lock,
    /// Drive a real host power verb (Suspend / Reboot / `PowerOff`) — the Power menu's
    /// host-down items, already the operator's typed-armed consent. The shell drives
    /// `system.honor_power(verb)` (the honorer's confirm-bypass seam, §6).
    Power(PowerVerb),
}

/// The live inputs the bottom **notification rail** folds — bundled into ONE
/// [`DockState`] field. Owned clones, refreshed each frame by the shell through
/// [`DockState::set_status_inputs`], so the vertical `dock(ctx, state)` needs no
/// extra parameters.
#[derive(Debug, Default)]
struct StatusInputs {
    /// The folded mesh summary from `chrome.rs`, kept so launcher badges can show
    /// live peer/health state without reopening the old top status strip.
    mesh: MeshSummary,
    /// The unified Chat unread count. Zero is meaningful silence and paints no
    /// badge.
    unread: usize,
    /// The `mde-seat` snapshot for NOTIF-4's device-control band, `None` pre-poll.
    seat: Option<SeatSnapshot>,
    /// Whether a VDI/Desktop session is currently requested or active.
    session_active: bool,
    /// Concrete Desktop sessions/requests to show as taskbar-style entries in the
    /// bottom rail. Empty preserves the old single dim Sessions glyph.
    sessions: Vec<SessionRailEntry>,
    /// NODE-GRADE-2 — the folded per-node capability grades the grade mini-list
    /// renders above the status strip (local pinned first, peers worst-first). The
    /// honest empty set pre-poll, so the band simply vanishes until grades arrive.
    grades: NodeGrades,
    /// NOTIF-3 — daemon-owned segment rollups rendered by the compact status
    /// strip. Missing rollups stay dim; the shell never fabricates green.
    segments: StatusSegments,
}

/// One open/detected Desktop session entry rendered in the bottom rail. This is a
/// display summary only; the Desktop/Chooser remains the source of truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRailEntry {
    /// Broker session id, when the row came from `action/vdi/session`. Fallback
    /// rows for a pending `VdiState` request have no id and still route Desktop.
    id: Option<String>,
    /// Human label, usually the VM/desktop name.
    label: String,
    /// Short protocol/status tag such as `RDP` or `VNC`.
    protocol: &'static str,
}

/// One compact Desktop source row rendered by the bottom rail flyout. It is a UI
/// summary only; `ChooserState` remains the source of truth and executes connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopRailSource {
    /// Stable chooser source id.
    pub(crate) id: String,
    /// Human label, usually VM/desktop name.
    pub(crate) label: String,
    /// Node/host label for the secondary line.
    node: String,
    /// Short protocol badge such as `RDP` or `VNC`.
    protocol: &'static str,
    /// Whether the row may be selected.
    pub(crate) connectable: bool,
    /// Whether the chooser prefs mark this source pinned/favorite.
    favorite: bool,
    /// Whether the chooser prefs mark this source recent.
    recent: bool,
}

impl DesktopRailSource {
    /// Construct a bounded compact row.
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

impl SessionRailEntry {
    /// Construct a bounded display entry. The label is kept short so a long VM
    /// name cannot consume the whole rail.
    pub fn new(label: impl Into<String>, protocol: &'static str) -> Self {
        Self::with_id(None, label, protocol)
    }

    /// Construct a bounded display entry backed by a broker session id.
    pub fn with_session_id(
        id: impl Into<String>,
        label: impl Into<String>,
        protocol: &'static str,
    ) -> Self {
        Self::with_id(Some(id.into()), label, protocol)
    }

    fn with_id(id: Option<String>, label: impl Into<String>, protocol: &'static str) -> Self {
        let label = truncate_session_label(&label.into());
        Self {
            id,
            label,
            protocol,
        }
    }
}

fn truncate_session_label(label: &str) -> String {
    const MAX_CHARS: usize = 24;
    let mut out = String::new();
    for ch in label.chars().take(MAX_CHARS) {
        out.push(ch);
    }
    if label.chars().count() > MAX_CHARS {
        out.push('…');
    }
    out
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

    /// Test seam for shell-level integration fixtures: mount the NOTIF-4 detail
    /// panel in the same frame as the status bar, edge cue, and Chat surface.
    #[cfg(test)]
    pub(crate) const fn open_status_panel_for_test(&mut self) {
        self.status_panel_open = true;
    }

    /// The **active surface** the app picker currently shows (VDOCK-2). The shell
    /// reads this back into its central view each frame after [`dock`] (the VDOCK-6
    /// wire) so a picker-cell click routes the shell body; [`Self::set_active`]
    /// mirrors the shell's live surface in first, so the picker highlights whatever
    /// surface is showing (design #25).
    pub const fn active(&self) -> Surface {
        self.active
    }

    /// Mirror the shell's live surface into the dock before [`dock`] (VDOCK-6) — a
    /// hotkey / chyron / self-test nav can move the surface OUTSIDE the picker, so
    /// the dock must track it (else the [`Self::active`] read-back would stomp that
    /// nav with a stale selection). A picker click then moves it and the shell reads
    /// it straight back.
    pub const fn set_active(&mut self, surface: Surface) {
        self.active = surface;
    }

    /// Mirror the Files transfer ledger's active count into the dock. The Files
    /// surface owns the ledger read; the dock only paints the count.
    pub const fn set_transfer_active_count(&mut self, count: usize) {
        self.transfer_active_count = count;
    }

    /// Mirror the shell-wide density into the dock. This is deliberately fed by
    /// the same formfactor path that installs [`Style`] density, so the shell has
    /// one compact/expanded mode instead of a second dock-local toggle.
    pub const fn set_density(&mut self, density: Density) {
        self.density = density;
    }

    /// Enable/disable the WIN10-HYBRID taskbar **auto-hide** setting (the shell feeds
    /// this from its persisted appearance config). When on, the bar reserves no
    /// bottom strut and reveals as a floating overlay on a bottom-edge hover (the B3
    /// reveal). Default off — the bar stays docked and reserves its strut.
    pub const fn set_taskbar_autohide(&mut self, on: bool) {
        self.taskbar_autohide = on;
    }

    /// Whether the taskbar is currently **hidden** (auto-hide on and not being
    /// revealed) — the predicate [`taskbar_strut_height`] keys off to drop the bottom
    /// strut to `0.0` (R5). (B3 folds the transient bottom-edge-hover reveal in here;
    /// today it tracks only the persisted auto-hide setting.)
    #[must_use]
    pub const fn taskbar_autohidden(&self) -> bool {
        self.taskbar_autohide
    }

    /// The bottom taskbar's height — the **fixed 48px** [`NOTIFICATION_RAIL_H`]
    /// (WIN10-HYBRID). Density-independent: density scales spacing + the hit-target
    /// floor, never this chrome dimension (design lock #7 / UX-24), so the same
    /// Windows-10-sized bar drives under every density. Kept a `&self` method (not an
    /// associated const) because it is called as `state.rail_height()` at ~5 sites and
    /// `start_menu.rs` reads it to reserve the SAME height above the bar for the Start
    /// grid's slide-up anchor — the WIN7-DESKTOP-1 regression fix — rather than a
    /// second, possibly-drifting guess.
    // Density-independent, but kept a `&self` method (called as `state.rail_height()`).
    #[allow(clippy::unused_self)]
    pub(crate) const fn rail_height(&self) -> f32 {
        NOTIFICATION_RAIL_H
    }

    /// Refresh the bottom **notification rail's** live inputs (NOTIF-3) — the shell calls
    /// this each frame before [`dock`] with the SAME folds the horizontal tray
    /// reads (`chrome.summary()`, `system.snapshot()`, `chat.total_unread()`, the
    /// live-session flag). Owned so the dock's `(ctx, state)` signature stays put;
    /// the quads render the pre-poll dim state until the first call lands (§7).
    /// Wired by `main.rs::mount_dock_chrome` (VDOCK-6) — the SOLE dock chrome.
    pub fn set_status_inputs(
        &mut self,
        mesh: MeshSummary,
        seat: Option<SeatSnapshot>,
        unread: usize,
        session_active: bool,
        sessions: Vec<SessionRailEntry>,
        grades: NodeGrades,
        segments: StatusSegments,
    ) {
        self.status = StatusInputs {
            mesh,
            unread,
            seat,
            session_active,
            sessions,
            grades,
            segments,
        };
    }

    /// Record a curtain-lock request (VDOCK-4) — the Lock system-quad cell. The
    /// dock never reaches the shell's `Curtain`; the shell drains this each frame
    /// ([`Self::take_request`]).
    const fn request_lock(&mut self) {
        self.pending = Some(DockRequest::Lock);
    }

    /// Fire a Power-menu item's REAL action + close the menu (VDOCK-4). Lock records
    /// a curtain request; every other item records its real [`PowerVerb`]. The
    /// typed-arming gate is the caller's — this fires unconditionally, so it runs
    /// only once Lock/Suspend are clicked, or a Reboot/Shutdown echo has armed.
    fn fire_power(&mut self, item: PowerItem) {
        // Lock → the shell curtain (NOT logind's session Lock, design #18); every
        // other item → its real logind verb.
        self.pending = Some(
            item.power_verb()
                .map_or(DockRequest::Lock, DockRequest::Power),
        );
        self.power.close();
    }

    /// Drain the pending shell **request** (VDOCK-4) — the shell calls this each
    /// frame after [`dock`] and drives it: a [`DockRequest::Lock`] drops the
    /// in-process curtain (`curtain.lock()`, exactly like Super+L), a
    /// [`DockRequest::Power`] drives `system.honor_power(verb)` (§6). `None` (drained
    /// once) otherwise. Wired by `main.rs::mount_dock_chrome` (VDOCK-6).
    pub const fn take_request(&mut self) -> Option<DockRequest> {
        self.pending.take()
    }

    /// Record a **node-focus** request (NODE-GRADE-2, design #7) — a grade row tap
    /// asking the shell to open that host's Explorer hero. The dock can't reach the
    /// Explorer / nav (§6); the shell drains this each frame ([`Self::take_node_focus`]).
    /// A fresh tap overwrites any un-drained one (the latest wins).
    fn request_node_focus(&mut self, host: &str) {
        self.pending_node_focus = Some(host.to_owned());
    }

    /// Drain the pending **node-focus** request (NODE-GRADE-2) — the shell calls this
    /// each frame after [`dock`] and, on `Some(host)`, routes to the Mesh Map's
    /// Explorer lens focused on that node (`ExplorerState::focus_node`, the reused
    /// EXPLORER jump path). `None` (drained once) otherwise. Wired by
    /// `main.rs::mount_dock_chrome`.
    pub const fn take_node_focus(&mut self) -> Option<String> {
        self.pending_node_focus.take()
    }

    /// Mirror the Start Menu panel's open state into the dock before [`dock`]
    /// (WIN7-2) — the Start cell's active tint then follows the real panel
    /// (the [`Self::set_active`] mirror idiom). Wired by
    /// `main.rs::mount_dock_chrome`.
    pub const fn set_start_menu_open(&mut self, open: bool) {
        self.start_menu_open = open;
    }

    /// Drain the Start cell's **Start Menu toggle** (WIN7-2) — `true` exactly
    /// once per Start-cell click; the shell flips the Start Menu panel on it
    /// (the [`Self::take_request`] deferred-wire idiom). Pressing Start with
    /// the panel already up drains through the same latch and closes it (lock
    /// #4, restated as WIN7-2's lock #13). A clean Super tap fires the SAME
    /// toggle through a different path — `main.rs` applies
    /// `crate::hotkeys::HotkeyRouter::take_dock_toggle`'s drain (the vertical
    /// dock's OWN pre-existing Super-tap latch) to this panel too; see
    /// `crate::start_menu`'s module doc for why one Super tap now reveals
    /// both.
    pub const fn take_start_menu_toggle(&mut self) -> bool {
        let toggled = self.start_menu_toggle;
        self.start_menu_toggle = false;
        toggled
    }

    /// Drain the bottom-rail Desktop reconnect request (NAVBAR-U1). This is
    /// separate from `active == Desktop` so programmatic navigation to Desktop does
    /// not silently initiate a reconnect.
    pub const fn take_desktop_reconnect(&mut self) -> bool {
        let reconnect = self.desktop_reconnect;
        self.desktop_reconnect = false;
        reconnect
    }

    /// Drain the compact Desktop flyout source selection (NAVBAR-U2).
    pub fn take_desktop_source_pick(&mut self) -> Option<String> {
        self.desktop_source_pick.take()
    }

    /// Drain the bottom-rail Desktop session focus selection (NAVBAR-U3).
    pub fn take_desktop_session_focus(&mut self) -> Option<String> {
        self.desktop_session_focus.take()
    }
}

/// Render the **left vertical dock** (VDOCK-1) — the slide-in, auto-hide chrome,
/// the shell's sole surface launcher. A left-edge, full-height [`DOCK_W`]
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
/// VDOCK-1 built the FRAME + the slide/toggle mechanism; **VDOCK-2** fills the
/// top **Workbench-lead** zone + the single-column **app-groups** middle; the
/// NODE-GRADE-2 grade band sits above **VDOCK-4**'s **system quad** in the final
/// row ([`paint_dock_frame`]). Returns `true` if a dock control routed this frame
/// — a picker cell, a grade row (a node-focus request), or a system-quad cell (a
/// route, the curtain lock, or the Power menu), recorded in [`DockState`] (the
/// active surface + the pending lock/power/node-focus requests) which the shell
/// reads back to surface the body / drive the seat. The auto-hide **pin**, the
/// **Start** cell, the session rail, the status tray, and the **clock** are NOT
/// part of this function — they render in the separate full-width bottom
/// taskbar ([`notification_rail_with_sources`], WIN7-1).
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
            let (claim, _claim) =
                ui.allocate_exact_size(egui::vec2(DOCK_W, screen.height()), egui::Sense::hover());
            let rect = egui::Rect::from_min_size(claim.min, egui::vec2(DOCK_W, claim.height()));
            clicked = paint_dock_frame(ui, rect, state);
        });

    // Keep frames flowing while the slide is in flight so the motion is smooth
    // (the curtain's tween idiom) — a no-op once settled at either end.
    if t > 0.001 && t < 0.999 {
        ctx.request_repaint();
    }
    clicked
}

/// Render the bottom **taskbar** (WIN7-1) — test-only convenience over
/// [`notification_rail_with_sources`] for callers with no live Desktop sources.
#[cfg(test)]
pub fn notification_rail(ctx: &egui::Context, state: &mut DockState) -> bool {
    notification_rail_with_sources(ctx, state, &[])
}

/// Render the shell's full-width **bottom taskbar** (design
/// `docs/design/win7-desktop-survey.md`, WIN7-1 lock #3), fed the compact Desktop
/// source flyout from `ChooserState` by the shell. Left → right: the **Start**
/// cell ([`start_cell`]) · the **running sessions** run (the Desktop rail cell +
/// source caret, then [`SessionRailEntry`]/[`DesktopRailSource`] entries or the
/// dim fallback glyph, NAVBAR-U1/U2/U3) · the **tray** (the status-detail
/// chevron + [`status::notification_rail`]'s segment pips, unchanged
/// click-through-to-Chat behavior) · the **clock** ([`clock_cell_rect`]) · the
/// auto-hide **pin** trailing last (this shell's own dock-hold-open control, not
/// a Win7 taskbar concept, so it rides past the four-part contract rather than
/// interrupting it). Compact (`Density::Mouse`, [`NOTIFICATION_RAIL_H`]) is this
/// taskbar's default, deliberately denser than the shell's other Carbon-baseline
/// chrome (lock #12); `Density::Touch` grows it to the labelled
/// [`NOTIFICATION_RAIL_EXPANDED_H`] variant.
pub fn notification_rail_with_sources(
    ctx: &egui::Context,
    state: &mut DockState,
    desktop_sources: &[DesktopRailSource],
) -> bool {
    let screen = ctx.screen_rect();
    let rail_h = state.rail_height();
    let rail_rect = egui::Rect::from_min_size(
        egui::pos2(screen.left(), screen.bottom() - rail_h),
        egui::vec2(screen.width(), rail_h),
    );
    let panel_t = Motion::animate(ctx, STATUS_PANEL_KEY, state.status_panel_open, Motion::BASE);
    let panel_top = rail_rect.top() - STATUS_PANEL_GAP - STATUS_PANEL_H
        + (1.0 - panel_t.clamp(0.0, 1.0)) * Style::SP_XL;
    let area_top = if panel_t > 0.001 {
        panel_top.min(rail_rect.top())
    } else {
        rail_rect.top()
    };
    let area_rect = egui::Rect::from_min_size(
        egui::pos2(screen.left(), area_top),
        egui::vec2(screen.width(), screen.bottom() - area_top),
    );
    let mut clicked = false;
    egui::Area::new(egui::Id::new(NOTIFICATION_RAIL_AREA))
        .order(egui::Order::Foreground)
        .fixed_pos(area_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(area_rect.size());
            // WIN7-DESKTOP-1 regression fix (see `docs/WORKLIST.md`): `ui.painter()`
            // (and `ui.interact`, used throughout this closure for every cell's hit
            // rect) always works in ABSOLUTE screen-space coordinates — an
            // `egui::Area`'s `fixed_pos` only seeds where the Area's own `Ui`
            // starts; egui establishes no separate (0,0)-based "local" frame for
            // content painted inside it (contrast `dock()` above, which hands
            // `paint_dock_frame` the rect it gets straight back from
            // `ui.allocate_exact_size` — already absolute, never re-derived). The
            // rail's own strip is always exactly `rail_rect` (already absolute,
            // computed above from `screen.bottom()`), regardless of whether the
            // Area grew upward this frame to also fit the animating status panel —
            // reuse it directly instead of re-deriving a rect offset by `area_top`.
            //
            // The pre-fix code built this as `Rect::from_min_size(pos2(0.0,
            // rail_rect.top() - area_top), rail_rect.size())`: in the rail's default
            // (no status-panel) state `area_top == rail_rect.top()`, so that
            // subtraction was always exactly `0.0` — pinning the WHOLE taskbar
            // (paint AND click hit-rects) at literal screen y=0 instead of the
            // bottom. Caught by WIN7-SHOT-1's screenshot harness; regression-tested
            // below (`win7_desktop_1_regression_the_taskbar_anchors_to_the_screens_
            // true_bottom_edge`) and by the analogous status-panel test, since
            // `notification_panel_rect` derives from this same rect.
            let local = rail_rect;
            ui.painter().rect_filled(
                local,
                egui::CornerRadius::ZERO,
                Style::BG.linear_multiply(0.92),
            );
            ui.painter().hline(
                local.left()..=local.right(),
                local.top(),
                egui::Stroke::new(HAIRLINE_W, Style::BORDER),
            );
            // WIN7-7, lock #14 — the taskbar itself needs a landmark role, not
            // just its contents: a screen reader jumping between landmarks
            // should be able to find "the taskbar" as its own stop, the same
            // way `start_menu.rs`'s `install_accessibility` gives the Start
            // Menu panel a `Role::Menu` landmark before any of ITS content is
            // covered.
            install_taskbar_accessibility(ui.ctx(), local);

            let mut x = local.left() + Style::SP_XS;
            let cell = |x: f32| {
                egui::Rect::from_min_size(egui::pos2(x, local.top()), egui::vec2(rail_h, rail_h))
                    .shrink(2.0)
            };

            if start_cell(ui, cell(x), state) {
                clicked = true;
            }
            x += rail_h;

            let desktop = cell(x);
            if rail_surface_cell(
                ui,
                Surface::Desktop,
                &mut state.active,
                &mut state.pinned,
                desktop,
                "Desktop",
            ) {
                state.desktop_reconnect = true;
                clicked = true;
            }
            x += rail_h;
            let source_caret = egui::Rect::from_min_size(
                egui::pos2(x, local.top()),
                egui::vec2(DESKTOP_CARET_W, rail_h),
            )
            .shrink(2.0);
            let opened_desktop_sources =
                desktop_source_toggle(ui, source_caret, state, desktop_sources.is_empty());
            if opened_desktop_sources {
                clicked = true;
            }
            x += DESKTOP_CARET_W;

            // Lock #3 (WIN7-1): Start · sessions · tray · clock, left to right — the
            // auto-hide pin is this shell's own dock-hold-open control (not a Win7
            // taskbar concept), so it trails past the clock as an extra rather than
            // interrupting the four-part contract (painted below, right to left).
            let tray_icon_w = rail_h.min(NOTIFICATION_RAIL_EXPANDED_ICON_H) - 4.0;
            let status_w = tray_icon_w * status::StatusSegment::ALL.len() as f32;
            let clock_w = rail_h * 2.2;
            let right_cluster_w =
                rail_h + clock_w + status_w + Style::SP_XS + rail_h + Style::SP_XS;
            let session_right = (local.right() - Style::SP_XS - right_cluster_w).max(x);
            if state.status.sessions.is_empty() {
                if rail_icon(
                    ui,
                    cell(x),
                    IconId::Sessions,
                    if state.status.session_active {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    },
                    "Sessions",
                    if state.status.session_active {
                        "Active"
                    } else {
                        "No active session"
                    },
                ) {
                    state.active = Surface::Desktop;
                    clicked = true;
                }
            } else {
                let sessions = state.status.sessions.clone();
                let mut sx = x;
                let mut focused_session = None;
                let overflow = rail_session_overflow(ui, &sessions, sx, session_right, rail_h);
                for (idx, entry) in sessions.iter().enumerate().take(overflow.visible) {
                    let desired = session_entry_width(ui, entry, rail_h);
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(sx, local.top()),
                        egui::vec2(desired, rail_h),
                    )
                    .shrink(2.0);
                    if session_entry(ui, rect, idx, entry, state.active == Surface::Desktop) {
                        state.active = Surface::Desktop;
                        focused_session.clone_from(&entry.id);
                        clicked = true;
                    }
                    sx += desired + Style::SP_XS;
                }
                if focused_session.is_some() {
                    state.desktop_session_focus = focused_session;
                }
                if overflow.hidden_start < sessions.len() {
                    let more = cell(sx);
                    let opened_more =
                        rail_more_cell(ui, more, state, sessions.len() - overflow.hidden_start);
                    if opened_more {
                        clicked = true;
                    }
                    if state.rail_more_open
                        && rail_more_popup(
                            ui,
                            more,
                            overflow.hidden_start,
                            &sessions,
                            state,
                            opened_more,
                            rail_h,
                        )
                    {
                        clicked = true;
                    }
                } else {
                    state.rail_more_open = false;
                }
            }
            // The pin is the taskbar's rightmost control (trailing the clock, see
            // the lock #3 note above) — this shell's own auto-hide affordance, not
            // one of the four locked taskbar segments.
            let mut tray_x = local.right() - Style::SP_XS - rail_h;
            if pin_toggle(ui, cell(tray_x), state) {
                clicked = true;
            }
            tray_x -= clock_w;
            if clock_cell_rect(
                ui,
                egui::Rect::from_min_size(
                    egui::pos2(tray_x, local.top()),
                    egui::vec2(clock_w, rail_h),
                )
                .shrink(2.0),
                state,
            ) {
                clicked = true;
            }
            tray_x -= status_w + Style::SP_XS;

            let pip_h = rail_h.min(NOTIFICATION_RAIL_EXPANDED_ICON_H) - 4.0;
            let status_rect = egui::Rect::from_min_size(
                // Vertically CENTER the pip band in the rail — the capped pip height
                // (≤24px) is shorter than the 48px bar, so top-aligning it (the old
                // `+2` that happened to centre a 14px pip in the 18px rail) leaves it
                // floating high. Centring holds the pips on the same row as the cells
                // at any rail height.
                egui::pos2(tray_x, local.top() + (rail_h - pip_h) / 2.0),
                egui::vec2(status_w, pip_h),
            );
            tray_x -= rail_h;
            if status_detail_toggle(ui, cell(tray_x), state) {
                clicked = true;
            }

            let mut active = state.active;
            let out = status::notification_rail(
                ui,
                &mut active,
                &state.status.grades,
                &state.status.segments,
                status_rect,
                state.status_panel_open,
            );
            state.active = active;
            if out.toggle_panel {
                state.status_panel_open = !state.status_panel_open;
                clicked = true;
            }
            if out.routed {
                clicked = true;
            }

            if panel_t > 0.001 {
                let panel_rect = notification_panel_rect(local, panel_t);
                let mut panel_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(panel_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                panel_ui.set_opacity(panel_t.clamp(0.0, 1.0));
                let panel_out = status::status_panel(
                    &panel_ui,
                    &state.status.grades,
                    state.status.seat.as_ref(),
                    panel_rect,
                );
                if panel_out.route_system {
                    state.active = Surface::System;
                    state.status_panel_open = false;
                    clicked = true;
                }
                if let Some(host) = panel_out.node_focus {
                    state.request_node_focus(&host);
                    state.status_panel_open = false;
                    clicked = true;
                }
                if status_panel_dismissed(ui, panel_rect, local) {
                    state.status_panel_open = false;
                    clicked = true;
                }
            }
            if state.desktop_sources_open
                && !opened_desktop_sources
                && desktop_source_flyout(ui, desktop, desktop_sources, state)
            {
                clicked = true;
            }
        });
    if panel_t > 0.001 && panel_t < 0.999 {
        ctx.request_repaint();
    }
    clicked
}

/// The width of the left **gutter** the shell reserves for the vertical dock this
/// frame (DOCK-OVERLAP) — [`DOCK_W`] scaled by the dock's live slide progress, so
/// the central content insets in lockstep with the sliding dock (no content jump
/// on reveal). Reads the SAME slide latch [`dock`] drives (idempotent within a
/// frame — egui's `animate_bool` returns the settled endpoint on first sight and
/// the same value on repeat reads), so the reserved gutter and the dock can never
/// drift apart. `0.0` when the dock is hidden **and settled** — the central
/// content then fills the full width. The shell reserves this as an empty left
/// gutter ONLY when NOT in a full-screen remote desktop; there the dock floats as
/// an overlay instead (`main.rs::central_view`).
pub fn gutter_width(ctx: &egui::Context, state: &DockState) -> f32 {
    let t = Motion::animate(ctx, DOCK_SLIDE_KEY, state.shown(), Motion::BASE);
    if t <= 0.001 {
        0.0
    } else {
        DOCK_W * t
    }
}

/// WIN10-HYBRID **bottom strut** — the height the shell reserves at the bottom edge
/// for the taskbar so surface content is never covered by it (the Windows-10 model:
/// a maximized surface ends *above* the taskbar, unlike the pre-hybrid floating
/// overlay). It is the taskbar's live [`rail_height`](DockState::rail_height).
/// Unlike the auto-hiding left-dock gutter this is reserved whenever the bar is
/// **docked**; when the bar is auto-hidden it returns `0.0` and the revealed bar
/// floats as an overlay instead (R5 — the strut is never eased with the reveal, so
/// content never jumps on a hover). Reserved by `main.rs::central_view` as an empty
/// bottom `TopBottomPanel` mounted before the `CentralPanel`.
#[must_use]
pub fn taskbar_strut_height(state: &DockState) -> f32 {
    if state.taskbar_autohidden() {
        0.0
    } else {
        state.rail_height()
    }
}

/// Paint the vertical dock's frame into `rect` and lay out its interior: the solid
/// Carbon-dark panel + the hairline right-edge divider (lock #24, §4 tokens), the
/// **VDOCK-2** top zone (the Workbench lead) and middle zone (the single-column
/// app groups + '…' overflow), the NODE-GRADE-2 grade band, and the **VDOCK-4**
/// system quad in the final `DOCK_W` row beneath it. The pin and the clock are
/// NOT painted here — both live in the bottom taskbar
/// ([`notification_rail_with_sources`], WIN7-1). Returns `true` if a picker cell,
/// a grade row, or a system-quad cell routed/acted this frame.
fn paint_dock_frame(ui: &mut egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let painter = ui.painter().clone();
    // Solid Carbon-dark panel fill (lock #24) — the SURFACE token (§4), a flat
    // opaque fill so the dock reads as one solid chrome column.
    painter.rect_filled(rect, egui::CornerRadius::ZERO, Style::SURFACE);
    // The hairline right-edge divider (lock #24) — a 1px BORDER rule down the
    // dock's right edge, the seam between the dock and the surface it floats over.
    // (The old DOCK-ACCENT blue edge seam was removed by operator directive.)
    painter.vline(
        rect.right(),
        rect.y_range(),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );

    let mut clicked = false;

    // ── TOP zone — the Workbench lead remains the first left-rail launcher. The
    // Start cell, Desktop shortcut, clock, and pin live in the bottom taskbar.
    let wb = egui::Rect::from_min_size(rect.min, egui::vec2(DOCK_W, DOCK_W));
    if pick_app_cell(
        ui,
        Surface::Workbench,
        &mut state.active,
        &mut state.pinned,
        wb,
    ) {
        clicked = true;
    }
    painter.hline(
        (rect.left() + Style::SP_XS)..=(rect.right() - Style::SP_XS),
        wb.bottom() + GROUP_DIVIDER_H / 2.0,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );

    // ── MIDDLE zone (design #2/#3/#4/#10/#11/#21/#22/#23) — the app picker ──
    // The six labelled groups stacked single-column top→bottom (Comms → … →
    // Media), each a horizontal accent label (#4) + a left-rail accent stripe +
    // accent divider (#21) over its icon-only 24px cells (#11/#23) in Surface::ALL
    // order (#3). The zone is bounded above the BOTTOM_ZONE_H band reserved for
    // VDOCK-5's clock strip + VDOCK-4's system quad; groups that
    // overrun it fold into the '…' more-popup (#22).
    // NODE-GRADE-2 — the per-node grade mini-list claims a band directly ABOVE the
    // bottom zone (between the app groups and the clock strip), so the app zone
    // now ends at the grade band's top. An empty grade set claims 0 (the band
    // vanishes and the groups reclaim the space, so pre-poll the layout is unchanged).
    let rail_h = state.rail_height();
    let quads_top_zone = rect.bottom() - rail_h - BOTTOM_ZONE_H;
    let grade_band_h = grade_band_height(&state.status.grades);
    let grade_top = quads_top_zone - grade_band_h;
    let middle_top = wb.bottom() + GROUP_DIVIDER_H;
    let middle_bottom = grade_top;
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
            &state.status,
            state.transfer_active_count,
            &mut state.active,
            &mut state.pinned,
        );
        if routed {
            clicked = true;
        }
        y += h;
    }
    if visible < GROUPS.len() && pick_overflow(ui, rect, middle_bottom, visible, &font, state) {
        clicked = true;
    }

    // ── GRADE band (NODE-GRADE-2 → design #4/#5/#6/#7/#8/#14/#15/#18/#19) — the
    // per-node capability grade mini-list, painted between the app groups and the
    // clock/system strip. Empty grades painted nothing (grade_band_h == 0).
    if grade_band_h > 0.0 && paint_grade_band(ui, rect, grade_top, state) {
        clicked = true;
    }

    // VDOCK-4 — the system quad in the reserved final DOCK_W row.
    let sys_top = rect.bottom() - rail_h - DOCK_W;
    painter.hline(
        (rect.left() + Style::SP_XS)..=(rect.right() - Style::SP_XS),
        sys_top,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
    if system_quad(ui, state, egui::pos2(rect.left(), sys_top), DOCK_W) {
        clicked = true;
    }

    clicked
}

/// The stable id of the Start Menu's trigger cell (WIN7-2; CONSOLE-1
/// originally), so tests read its settled `Rect`.
fn start_cell_id() -> egui::Id {
    egui::Id::new("vdock-start-cell")
}

/// Stable id for a bare icon-only taskbar cell ([`rail_icon`]) — keyed by the
/// glyph itself since (today) each [`IconId`] only ever backs one such cell.
fn rail_icon_id(icon: IconId) -> egui::Id {
    egui::Id::new(("bottom-rail-icon", icon.name()))
}

/// A bare icon-only taskbar cell (WIN7-1's sessions-empty fallback glyph is
/// its one caller today). `label`/`value` are the WIN7-7 accesskit pair
/// (lock #14) — every other taskbar cell function exports its own `Button`
/// node ([`install_cell_accessibility`]); this one is no exception just
/// because it's the generic/shared shape.
fn rail_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    icon: IconId,
    tint: egui::Color32,
    label: &str,
    value: &str,
) -> bool {
    let resp = ui.interact(rect, rail_icon_id(icon), egui::Sense::click());
    let color = if resp.hovered() { Style::TEXT } else { tint };
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let edge = (rect.height() - 2.0).max(Style::SP_S);
    if let Some(tex) = icon_texture(ui.ctx(), icon, edge, color) {
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(edge, edge));
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }
    paint_focus_ring(ui.painter(), rect, resp.has_focus());
    install_cell_accessibility(ui.ctx(), rail_icon_id(icon), label, value, rect);
    resp.clicked()
}

fn paint_rail_label(ui: &egui::Ui, rect: egui::Rect, label: &str, tint: egui::Color32) {
    if rect.height() < NOTIFICATION_RAIL_EXPANDED_H - 1.0 {
        return;
    }
    ui.painter().text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_XS),
        egui::Align2::CENTER_BOTTOM,
        label,
        egui::FontId::proportional(Style::SMALL),
        tint,
    );
}

fn rail_icon_rect(rect: egui::Rect, edge: f32) -> egui::Rect {
    let y = if rect.height() >= NOTIFICATION_RAIL_EXPANDED_H - 1.0 {
        rect.top() + Style::SP_XS + edge / 2.0
    } else {
        rect.center().y
    };
    egui::Rect::from_center_size(egui::pos2(rect.center().x, y), egui::vec2(edge, edge))
}

fn rail_surface_cell(
    ui: &egui::Ui,
    surface: Surface,
    active: &mut Surface,
    pinned: &mut bool,
    rect: egui::Rect,
    label: &str,
) -> bool {
    let selected = *active == surface;
    let resp = ui.interact(rect, pick_cell_id(surface), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if selected {
        let bar =
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(ACTIVE_BAR_W, rect.height()));
        painter.rect_filled(bar, egui::CornerRadius::ZERO, Style::ACCENT);
    }
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, rect, label, tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    apply_picker_arrow_focus(ui, surface, &resp);
    paint_surface_context_menu(ui, surface, &resp, active, pinned);
    install_cell_accessibility(
        ui.ctx(),
        pick_cell_id(surface),
        label,
        if selected { "Active" } else { "Not active" },
        rect,
    );
    if response_activated(ui, &resp) {
        *active = surface;
        return true;
    }
    false
}

fn session_entry_id(idx: usize, entry: &SessionRailEntry) -> egui::Id {
    egui::Id::new((
        "bottom-rail-session",
        idx,
        entry.id.as_deref(),
        entry.label.as_str(),
        entry.protocol,
    ))
}

fn session_entry_width(ui: &egui::Ui, entry: &SessionRailEntry, rail_h: f32) -> f32 {
    let text = format!("{} {}", entry.label, entry.protocol);
    let font = egui::FontId::proportional(Style::SMALL);
    let text_w = ui.fonts(|f| f.layout_no_wrap(text, font, Style::TEXT).rect.width());
    (rail_h + text_w + Style::SP_S).clamp(rail_h * 2.0, 180.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RailSessionOverflow {
    visible: usize,
    hidden_start: usize,
}

fn rail_more_id() -> egui::Id {
    egui::Id::new("bottom-rail-more")
}

fn rail_more_popup_id() -> egui::Id {
    egui::Id::new("bottom-rail-more-popup")
}

fn rail_session_overflow(
    ui: &egui::Ui,
    sessions: &[SessionRailEntry],
    start_x: f32,
    right_x: f32,
    rail_h: f32,
) -> RailSessionOverflow {
    let available = (right_x - start_x).max(0.0);
    let widths: Vec<f32> = sessions
        .iter()
        .map(|entry| session_entry_width(ui, entry, rail_h))
        .collect();
    let total: f32 = widths
        .iter()
        .enumerate()
        .map(|(idx, width)| {
            if idx + 1 == widths.len() {
                *width
            } else {
                *width + Style::SP_XS
            }
        })
        .sum();
    if total <= available {
        return RailSessionOverflow {
            visible: sessions.len(),
            hidden_start: sessions.len(),
        };
    }

    let more_w = rail_h + Style::SP_XS;
    let mut used = 0.0;
    let mut visible = 0;
    for width in widths {
        let next = if visible == 0 {
            width
        } else {
            Style::SP_XS + width
        };
        if used + next + more_w > available {
            break;
        }
        used += next;
        visible += 1;
    }
    RailSessionOverflow {
        visible,
        hidden_start: visible,
    }
}

fn session_entry(
    ui: &egui::Ui,
    rect: egui::Rect,
    idx: usize,
    entry: &SessionRailEntry,
    selected: bool,
) -> bool {
    let resp = ui.interact(rect, session_entry_id(idx, entry), egui::Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if resp.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if selected || resp.hovered() {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    };
    let icon_edge = (rect.height() - 2.0).max(Style::SP_S);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + Style::SP_XS,
            rect.center().y - icon_edge / 2.0,
        ),
        egui::vec2(icon_edge, icon_edge),
    );
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Sessions, icon_edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }
    let text_x = icon_rect.right() + Style::SP_XS;
    if text_x < rect.right() {
        let clip = egui::Rect::from_min_max(egui::pos2(text_x, rect.top()), rect.right_bottom());
        let text = format!("{} {}", entry.label, entry.protocol);
        painter.with_clip_rect(clip).text(
            egui::pos2(text_x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(Style::SMALL),
            if selected || resp.hovered() {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
        );
    }
    paint_focus_ring(&painter, rect, resp.has_focus());
    // WIN7-7, lock #14 — unconditional (unlike the visual text above, which
    // clips away entirely when the cell is too narrow): a screen reader
    // needs the session's name every time, not only when there happens to
    // be pixel room for it.
    install_cell_accessibility(
        ui.ctx(),
        session_entry_id(idx, entry),
        format!("{} {}", entry.label, entry.protocol),
        if selected {
            "Active desktop session"
        } else {
            "Desktop session"
        },
        rect,
    );
    resp.clicked()
}

/// Stable id for the bottom-rail status detail toggle.
fn status_detail_toggle_id() -> egui::Id {
    egui::Id::new(("bottom-rail-icon", IconId::ChevronUp.name()))
}

fn status_detail_toggle(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, status_detail_toggle_id(), egui::Sense::click());
    let selected = state.status_panel_open;
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let edge = (rect.height() - 2.0).max(Style::SP_S);
    if let Some(tex) = icon_texture(ui.ctx(), IconId::ChevronUp, edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, rect, "Status", tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        status_detail_toggle_id(),
        "Notification panel",
        if selected { "Expanded" } else { "Collapsed" },
        rect,
    );
    if resp.clicked() {
        state.status_panel_open = !state.status_panel_open;
        return true;
    }
    false
}

/// The **Start cell** — the Start Menu's trigger (WIN7-2, design locks
/// #4/#13; CONSOLE-1's original Start front door, relabelled "Start" per
/// WIN7-1): the bottom taskbar's far-left affordance, wearing the repo's
/// Win10-style Start/Menu tray glyph. A click latches the Start Menu toggle
/// for the shell to drain ([`DockState::take_start_menu_toggle`] — the
/// deferred wire; pressing it again closes, lock #4/#13). While the panel is
/// up (mirrored in via [`DockState::set_start_menu_open`]) the cell wears the
/// selection wash + ACCENT tint, the sys-cell "menu open" idiom. Every colour
/// is a Style token (§4). Returns `true` on a click.
fn start_cell(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, start_cell_id(), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if state.start_menu_open {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if state.start_menu_open {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let icon_edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Start, icon_edge, tint) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, icon_edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, rect, "Start", tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        start_cell_id(),
        "Start",
        if state.start_menu_open {
            "Start Menu open"
        } else {
            "Start Menu closed"
        },
        rect,
    );
    if resp.clicked() {
        state.start_menu_toggle = true;
        return true;
    }
    false
}

/// The dock's **pin** toggle (VDOCK-1, lock #9) — the minimal affordance that
/// holds the dock open when set (the "pin" half of "hotkey + pin, no hover").
/// It uses the repo's shared tray pin glyph so the bottom rail remains all-icons.
/// Every colour is a Style token (§4). Returns `true` on a click (which flips the
/// pin via [`DockState::toggle_pin`]).
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
    if resp.hovered() {
        ui.painter()
            .rect_filled(cell, Style::RADIUS, Style::SURFACE_HI);
    }
    let edge = ICON_LOGICAL.min((cell.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Pin, edge, color) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter().image(
            tex.id(),
            rail_icon_rect(cell, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, cell, "Pin", color);
    paint_focus_ring(ui.painter(), cell, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        egui::Id::new("vdock-pin"),
        "Pin",
        if pinned { "Pinned" } else { "Not pinned" },
        cell,
    );
    if resp.clicked() {
        state.toggle_pin();
        return true;
    }
    false
}

/// The stable id of the clock strip (VDOCK-5), so tests read its settled `Rect`.
fn clock_cell_id() -> egui::Id {
    egui::Id::new("vdock-clock")
}

/// The WIN10-HYBRID tray clock's second line — the civil date `M/D/YYYY` for the
/// `now_unix` timestamp, formatted through the crate's ONE calendar
/// ([`crate::chat::civil_from_days`], §6, so no second date fold leaks in). Pure —
/// unit-testable without a painter.
fn clock_date_text(now_unix: i64) -> String {
    let (year, month, day) = crate::chat::civil_from_days(now_unix.div_euclid(86_400));
    format!("{month}/{day}/{year}")
}

/// The **clock strip** (VDOCK-5, locks #16/#20) — the status cell whose glyph IS
/// the live wall-clock `HH:MM` (painted text through the crate's one clock fold,
/// `crate::timers::hhmm` — the brand set has no clock glyph and the design wants
/// the *time* read as the icon). It reads as a clock and routes to the **Timers
/// & Alarms** surface on click (`Surface::Timers`), wearing the same selection
/// wash + left-edge accent bar as an app cell (#10). Every colour is a Style
/// token (§4). Self-schedules a repaint at the next minute rollover so the
/// painted minute is never stale. Returns `true` on a route.
fn clock_cell_rect(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, clock_cell_id(), egui::Sense::click());
    let selected = state.active == Surface::Timers;
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if selected {
        let bar =
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(ACTIVE_BAR_W, rect.height()));
        painter.rect_filled(bar, egui::CornerRadius::ZERO, Style::ACCENT);
    }
    // The pick_app_cell two-tone: active reads ACCENT, hover brightens to TEXT,
    // rest sits dim — the time digits ARE the glyph (lock #20).
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let now = crate::timers::now_unix();
    let time_text = crate::timers::hhmm(now);
    let time_font = egui::FontId::proportional(Style::SMALL.min((rect.height() - 6.0).max(8.0)));
    if rect.height() >= NOTIFICATION_RAIL_EXPANDED_H - 1.0 {
        // WIN10-HYBRID — the Win10 tray clock is two lines: HH:MM over the date. On
        // the 48px bar the date replaces the old single "Clock" label; the crate's ONE
        // calendar (`chat::civil_from_days`) formats it so no second date fold leaks in.
        painter.text(
            egui::pos2(rect.center().x, rect.center().y - Style::SP_XS - 1.0),
            egui::Align2::CENTER_CENTER,
            &time_text,
            time_font,
            tint,
        );
        painter.text(
            egui::pos2(rect.center().x, rect.center().y + Style::SP_S),
            egui::Align2::CENTER_CENTER,
            &clock_date_text(now),
            egui::FontId::proportional(Style::SMALL - 1.0),
            if selected {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            },
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &time_text,
            time_font,
            tint,
        );
    }
    paint_focus_ring(&painter, rect, resp.has_focus());
    // WIN7-7, lock #14 — the clock's accessible VALUE is the same live
    // `HH:MM` reading its glyph paints (the task's own "does the clock
    // announce the time" question): a screen reader can navigate to the
    // clock and hear the time on demand, exactly like a real desktop clock.
    // Deliberately NOT a `Live::Polite` region — that would narrate the
    // clock out loud on every unattended minute rollover, which nobody
    // wants (the `install_tile_accessibility` "not individually live, would
    // be a spam regression" precedent, restated here for a once-a-minute
    // rather than a once-per-rotation cadence).
    install_cell_accessibility(ui.ctx(), clock_cell_id(), "Clock", &time_text, rect);
    // Wake at the next minute rollover so the glyph never shows a stale minute
    // (cheap: egui keeps only the earliest scheduled repaint).
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(
            crate::timers::secs_to_next_minute(now).max(1),
        ));
    if resp.clicked() {
        state.active = Surface::Timers;
        return true;
    }
    false
}

// ── accesskit (lock #14, WIN7-7) ─────────────────────────────────────────────
//
// Before this unit, `dock.rs` exported NOTHING to the accessibility tree: every
// cell above is a hand-rolled `ui.interact(rect, id, sense)` widget, and egui
// only auto-generates accesskit nodes for real widgets (`TextEdit`, `Button`)
// via `Response::widget_info` — never for raw `interact`-based cells (verified
// against this crate: zero `widget_info` call sites in this file, the ONE
// production call anywhere in the crate's dock/console/status/start_menu
// group is `status.rs`'s own `#[cfg(test)]`-only retired `status_bar`). The
// bottom taskbar's tray pips were always covered (`status.rs`'s
// `install_segment_accessibility`, called per-segment from
// `notification_rail`) — this section is what closes the REST of the gap:
// the taskbar's own landmark, and every taskbar-owned cell (Start, the
// Desktop rail cell, the Desktop-source caret + its flyout rows, session
// entries + the overflow "more" popup — which reuses [`session_entry`], so
// one fix covers both — the status/notification toggle, the clock, and the
// pin). Restates the SAME `accesskit_rect` helper + `Role::Button` +
// label/value/bounds/`Click` shape `status.rs`/`console.rs`/`start_menu.rs`
// already use (each panel's accesskit section owns its own copy, the
// established per-module convention), named `install_cell_accessibility`
// here since this module's own vocabulary for one of these is a "cell", not
// a "row" (`console.rs`) or a "tile" (`start_menu.rs`).

/// Convert an egui rect to an accesskit one (the `status.rs`/`console.rs`/
/// `start_menu.rs` helper, restated module-locally per this crate's
/// established per-module-copy convention).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// Stable id for the taskbar's own landmark node.
fn taskbar_accesskit_id() -> egui::Id {
    egui::Id::new("bottom-rail-taskbar-accesskit")
}

/// Install the taskbar's own landmark (lock #14 — "the taskbar itself", not
/// just its contents): `Role::Toolbar` is accesskit's ARIA-`toolbar`-
/// equivalent role for "a container grouping a set of controls," the exact
/// shape of a Win7-style taskbar — matching how a real desktop taskbar
/// exposes itself to Windows UI Automation. Mirrors `start_menu.rs`'s own
/// `install_accessibility` giving the Start Menu panel a landmark role
/// before any of its content is individually covered.
fn install_taskbar_accessibility(ctx: &egui::Context, rect: egui::Rect) {
    let _ = ctx.accesskit_node_builder(taskbar_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Toolbar);
        node.set_label("Taskbar");
        node.set_bounds(accesskit_rect(rect));
    });
}

/// Install one taskbar cell's accesskit `Button` node: role + a fixed
/// `label` (the control's identity) + a dynamic `value` (its current
/// state/reading) + bounds + the `Click` action — the SAME shape
/// `status.rs`'s `install_segment_accessibility` / `console.rs`'s
/// `install_row_accessibility` / `start_menu.rs`'s `install_tile_accessibility`
/// already use, restated here for this module's own raw-painted cells.
fn install_cell_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    label: impl Into<String>,
    value: impl Into<String>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.into());
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
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

/// The horizontal accent-label row above each group (#4) — `SP_M` tall, its label
/// sized to fit the narrow column by [`group_label_font`].
const PICK_LABEL_H: f32 = Style::SP_M;

/// The per-group **separation** band (#21) — an `SP_S` gap between one group's
/// full accent outline box and the next, so each colored box reads as its own
/// enclosed cluster.
const GROUP_DIVIDER_H: f32 = Style::SP_S;

/// The active cell's **left-edge accent bar** (lock #10) — an `SP_XS`-wide
/// [`Style::ACCENT`] bar down the active surface's left edge (the vertical analog
/// of the horizontal bar's bottom underline), at the cell's outer edge.
const ACTIVE_BAR_W: f32 = Style::SP_XS;

/// The '…' overflow cell height (#22) — the more-popup trigger at the bottom of
/// the app zone. `SP_L`.
const OVERFLOW_H: f32 = Style::SP_L;

/// The bottom band reserved beneath the app zone: only VDOCK-4's system row
/// remains in the left rail. Start, Desktop, Clock, Pin, and notification status
/// micro-icons live in the full-width bottom taskbar.
const BOTTOM_ZONE_H: f32 = DOCK_W;

/// The full-width bottom taskbar's height — a **fixed 48px** matching the Windows-10
/// taskbar (WIN10-HYBRID; `SP_XL + SP_M` on the 8px grid). Density-independent
/// (design lock #7 / UX-24 — density scales spacing + the hit-target floor, never
/// this chrome dimension), so every density drives the same Win10-sized bar. At 48px
/// each square cell is 48×48 and its icon reaches the full [`ICON_LOGICAL`] 24px cap;
/// content reserves this as the bottom strut (`taskbar_strut_height`) rather than
/// being overlaid.
const NOTIFICATION_RAIL_H: f32 = Style::SP_XL + Style::SP_M;
/// NAVBAR-8's expanded bar height: touch/expanded density grows the rail to a
/// labelled Win10-style taskbar variant while compact density keeps the denser
/// [`NOTIFICATION_RAIL_H`] rail. Deliberately left at the standard [`DOCK_W`]
/// scale (unlike the compact rail) — `Density::Touch` exists specifically for
/// larger touch targets, so WIN7-1's density pass does not shrink it.
const NOTIFICATION_RAIL_EXPANDED_H: f32 = 48.0;
const NOTIFICATION_RAIL_EXPANDED_ICON_H: f32 = 24.0;
const DESKTOP_CARET_W: f32 = 14.0;
const DESKTOP_SOURCE_ROW_H: f32 = 28.0;
const DESKTOP_SOURCE_FLYOUT_W: f32 = Style::SP_XL * 7.5;
const DESKTOP_SOURCE_MAX_ROWS: usize = 8;

/// NOTIF-4's right slide-out width: compact enough to stay auxiliary, wide enough
/// for grade names and three device meters.
const STATUS_PANEL_W: f32 = Style::SP_XL * 7.0;
const STATUS_PANEL_GAP: f32 = Style::SP_XS;
const STATUS_PANEL_H: f32 = Style::SP_XL * 8.0;

fn notification_panel_rect(rail: egui::Rect, t: f32) -> egui::Rect {
    let left = rail.left() + Style::SP_S;
    let top =
        rail.top() - STATUS_PANEL_GAP - STATUS_PANEL_H + (1.0 - t.clamp(0.0, 1.0)) * Style::SP_XL;
    egui::Rect::from_min_size(
        egui::pos2(left, top),
        egui::vec2(STATUS_PANEL_W, STATUS_PANEL_H),
    )
}

fn status_panel_dismissed(ui: &egui::Ui, panel_rect: egui::Rect, status_rect: egui::Rect) -> bool {
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return true;
    }
    ui.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !panel_rect.contains(pos) && !status_rect.contains(pos))
    })
}

fn desktop_source_toggle_id() -> egui::Id {
    egui::Id::new("bottom-rail-desktop-source-toggle")
}

fn desktop_source_row_id(source: &DesktopRailSource) -> egui::Id {
    egui::Id::new(("bottom-rail-desktop-source", source.id.as_str()))
}

fn desktop_source_flyout_id() -> egui::Id {
    egui::Id::new("bottom-rail-desktop-source-flyout")
}

fn desktop_source_toggle(
    ui: &egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    empty: bool,
) -> bool {
    let resp = ui.interact(rect, desktop_source_toggle_id(), egui::Sense::click());
    let hovered = resp.hovered();
    let selected = state.desktop_sources_open;
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if selected {
        Style::ACCENT
    } else if hovered {
        Style::TEXT
    } else if empty {
        Style::TEXT_DIM.linear_multiply(0.7)
    } else {
        Style::TEXT_DIM
    };
    let edge = (rect.height() - 2.0).max(Style::SP_S);
    if let Some(tex) = icon_texture(ui.ctx(), IconId::ChevronUp, edge, tint) {
        let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(edge, edge));
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        desktop_source_toggle_id(),
        "Desktop sources",
        if empty {
            "No desktop sources available"
        } else if selected {
            "Expanded"
        } else {
            "Collapsed"
        },
        rect,
    );
    if response_activated(ui, &resp) {
        state.desktop_sources_open = !state.desktop_sources_open;
        return true;
    }
    false
}

fn desktop_source_flyout(
    ui: &egui::Ui,
    anchor: egui::Rect,
    sources: &[DesktopRailSource],
    state: &mut DockState,
) -> bool {
    let rows = sources.len().clamp(1, DESKTOP_SOURCE_MAX_ROWS);
    let popup_h = rows as f32 * DESKTOP_SOURCE_ROW_H + Style::SP_S;
    let inner = egui::Area::new(desktop_source_flyout_id())
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(anchor.left(), anchor.top() - Style::SP_XS))
        .show(ui.ctx(), |ui| {
            let (area, _) = ui.allocate_exact_size(
                egui::vec2(DESKTOP_SOURCE_FLYOUT_W, popup_h),
                egui::Sense::hover(),
            );
            let bg = ui.painter().add(egui::Shape::Noop);
            let mut picked = None;
            if sources.is_empty() {
                paint_empty_desktop_sources(ui, area);
            } else {
                let mut y = area.top() + Style::SP_XS / 2.0;
                for source in sources.iter().take(DESKTOP_SOURCE_MAX_ROWS) {
                    let row = egui::Rect::from_min_size(
                        egui::pos2(area.left() + Style::SP_XS, y),
                        egui::vec2(DESKTOP_SOURCE_FLYOUT_W - Style::SP_S, DESKTOP_SOURCE_ROW_H),
                    );
                    if desktop_source_row(ui, row, source) {
                        picked = Some(source.id.clone());
                    }
                    y += DESKTOP_SOURCE_ROW_H;
                }
            }
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(area, Style::RADIUS, Style::SURFACE),
            );
            ui.painter().rect_stroke(
                area,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            picked
        });

    if let Some(id) = inner.inner {
        state.desktop_source_pick = Some(id);
        state.desktop_sources_open = false;
        state.active = Surface::Desktop;
        return true;
    }
    if inner.response.clicked_elsewhere() {
        state.desktop_sources_open = false;
        return true;
    }
    false
}

fn paint_empty_desktop_sources(ui: &egui::Ui, area: egui::Rect) {
    ui.painter().text(
        area.center(),
        egui::Align2::CENTER_CENTER,
        "No desktop sources",
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

fn desktop_source_row(ui: &egui::Ui, rect: egui::Rect, source: &DesktopRailSource) -> bool {
    let resp = ui.interact(
        rect,
        desktop_source_row_id(source),
        if source.connectable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let painter = ui.painter().clone();
    if resp.hovered() && source.connectable {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if !source.connectable {
        Style::TEXT_DIM.linear_multiply(0.65)
    } else if source.favorite {
        Style::ACCENT
    } else if source.recent {
        Style::OK
    } else {
        Style::TEXT_DIM
    };
    let icon_edge = (rect.height() - Style::SP_XS).max(Style::SP_M);
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + Style::SP_M, rect.center().y),
        egui::vec2(icon_edge, icon_edge),
    );
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Desktop, icon_edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }
    let text_x = icon_rect.right() + Style::SP_XS;
    let clip = egui::Rect::from_min_max(egui::pos2(text_x, rect.top()), rect.right_bottom());
    painter.with_clip_rect(clip).text(
        egui::pos2(text_x, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        source.label.as_str(),
        egui::FontId::proportional(Style::SMALL),
        if source.connectable {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    painter.with_clip_rect(clip).text(
        egui::pos2(text_x, rect.bottom() - Style::SP_XS),
        egui::Align2::LEFT_BOTTOM,
        format!("{} {}", source.node, source.protocol),
        egui::FontId::proportional((Style::SMALL - 1.0).max(8.0)),
        Style::TEXT_DIM,
    );
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        desktop_source_row_id(source),
        source.label.as_str(),
        if source.connectable {
            format!("{} {}", source.node, source.protocol)
        } else {
            format!("{} {} (unavailable)", source.node, source.protocol)
        },
        rect,
    );
    source.connectable
        && (response_activated(ui, &resp)
            || (resp.hovered() && ui.input(|i| i.pointer.any_released())))
}

/// The stable per-surface id of a vertical-picker cell — the render + routing are
/// unchanged, but tests read a cell's settled `Rect` back to click its exact
/// centre (the addressable-cell idiom, so a picker cell never shares an id with a
/// status/system-quad cell).
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
fn pick_app_cell(
    ui: &egui::Ui,
    surface: Surface,
    active: &mut Surface,
    pinned: &mut bool,
    rect: egui::Rect,
) -> bool {
    pick_app_cell_with_badge(ui, surface, active, pinned, rect, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BadgeKind {
    Count(usize),
    Health(BadgeTone),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BadgeTone {
    Healthy,
    Degraded,
    Offline,
}

fn badge_for(
    surface: Surface,
    status: &StatusInputs,
    transfer_active_count: usize,
) -> Option<BadgeKind> {
    match surface {
        Surface::Files if transfer_active_count > 0 => {
            Some(BadgeKind::Count(transfer_active_count))
        }
        Surface::Chat if status.unread > 0 => Some(BadgeKind::Count(status.unread)),
        Surface::MeshView if status.mesh.seen && status.mesh.peers_total > 0 => {
            Some(BadgeKind::Count(status.mesh.peers_online))
        }
        Surface::System if status.mesh.seen => Some(BadgeKind::Health(match status.mesh.health {
            mde_lighthouse_health::LighthouseHealth::AllHealthy => BadgeTone::Healthy,
            mde_lighthouse_health::LighthouseHealth::Degraded => BadgeTone::Degraded,
            mde_lighthouse_health::LighthouseHealth::None => BadgeTone::Offline,
        })),
        _ => None,
    }
}

fn pick_app_cell_with_badge(
    ui: &egui::Ui,
    surface: Surface,
    active: &mut Surface,
    pinned: &mut bool,
    rect: egui::Rect,
    badge: Option<BadgeKind>,
) -> bool {
    let selected = *active == surface;
    // The app cells are icon-only (no visible caption), so the surface's own
    // label rides a plain hover tooltip (house style, `Response::on_hover_text`)
    // — a user can learn what an unlabelled cell does without any persistent
    // label that would shift this fixed-grid layout. Delegated `pick_app_cell`
    // routes through here, so both entry points get the tooltip.
    let resp = ui
        .interact(rect, pick_cell_id(surface), egui::Sense::click())
        .on_hover_text(surface.label());
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
    if let Some(badge) = badge {
        paint_badge(ui, rect, surface, badge);
    }
    paint_focus_ring(&painter, rect, resp.has_focus());
    apply_picker_arrow_focus(ui, surface, &resp);
    paint_surface_context_menu(ui, surface, &resp, active, pinned);

    if response_activated(ui, &resp) {
        *active = surface;
        return true;
    }
    false
}

/// Apply this cell's own one-step `PICKER_FOCUS_ORDER` arrow-key navigation
/// (NAVBAR-6). Makes this function the SOLE authority over arrow-key focus
/// movement among the picker cells — matching what every existing behavioral
/// test already assumed — by neutralizing TWO independent ways an arrow key
/// press could otherwise move focus more than the intended one step.
///
/// WIN7-DESKTOP-1 regression fix (see `docs/WORKLIST.md`): investigating the
/// taskbar-position fix's fallout found this function was never actually
/// robust — it just happened to LOOK correct because the pre-fix bottom-rail
/// Desktop cell (mispositioned at literal screen y≈0) coincidentally masked
/// both bugs below; confirmed by reverting the taskbar-position fix in
/// isolation and observing these SAME two mechanisms fire, unchanged.
///
/// 1. **Egui's own built-in spatial arrow-key nav races this function.**
///    Vendored egui 0.31.1's `memory/mod.rs`, `Focus::begin_pass`/`end_pass`:
///    `begin_pass` latches a cardinal `FocusDirection` for any arrow key the
///    CURRENTLY-focused widget's `EventFilter` doesn't claim, and `end_pass`
///    then unconditionally overwrites `focused_widget` with whatever its own
///    `find_widget_in_direction` spatial search finds — a raw screen-position
///    search that knows nothing about `PICKER_FOCUS_ORDER`. Pre-fix, that
///    spatial search (from the app-picker column, searching "down") landed on
///    the SAME cell this function's own table-driven logic wanted, by pure
///    positional coincidence; post-fix there is nothing spatially "below" the
///    true-bottom taskbar, so the search finds nothing and end_pass leaves
///    focus wherever mechanism 2 below left it. Fixed by claiming vertical +
///    horizontal arrows via `set_focus_lock_filter` — egui's own documented
///    seam for "I handle these keys myself" — every frame this cell holds
///    focus, so `begin_pass` never latches a direction in the first place.
/// 2. **This function's OWN `request_focus` call can cascade in one frame.**
///    `ui.input(|i| i.key_pressed(...))` doesn't consume the event — it stays
///    "pressed" for the rest of the frame. So the cell focus just moved TO
///    (rendered later this same frame, e.g. the next `PICKER_FOCUS_ORDER`
///    entry) sees `resp.has_focus() == true` (this call just set it) AND the
///    SAME still-pressed key, and moves focus again — and so on, potentially
///    through the WHOLE table in one frame. Fixed by consuming every arrow
///    key event the moment a move actually fires, so no widget rendered later
///    this same frame can see it "pressed" again.
fn apply_picker_arrow_focus(ui: &egui::Ui, surface: Surface, resp: &egui::Response) {
    if !resp.has_focus() {
        return;
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            pick_cell_id(surface),
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..egui::EventFilter::default()
            },
        );
    });
    let dir = ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowRight) {
            Some(1)
        } else if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowLeft) {
            Some(-1)
        } else {
            None
        }
    });
    if let Some(dir) = dir.and_then(|d| picker_focus_neighbor(surface, d)) {
        ui.memory_mut(|m| m.request_focus(pick_cell_id(dir)));
        // Consume every arrow key this frame (mechanism 2 above) — key-only
        // matching (no modifier filter), mirroring `key_pressed`'s own match
        // above exactly; `InputState::consume_key` needs an EXACT modifier
        // match, which would silently stop claiming e.g. Shift+ArrowDown, a
        // real (if narrow) behavior change this regression fix doesn't need.
        ui.input_mut(|i| {
            i.events.retain(|ev| {
                !matches!(
                    ev,
                    egui::Event::Key { key, pressed: true, .. }
                    if matches!(
                        key,
                        egui::Key::ArrowDown
                            | egui::Key::ArrowRight
                            | egui::Key::ArrowUp
                            | egui::Key::ArrowLeft
                    )
                )
            });
        });
    }
}

fn picker_focus_neighbor(surface: Surface, dir: i32) -> Option<Surface> {
    let idx = PICKER_FOCUS_ORDER.iter().position(|&s| s == surface)?;
    let next = if dir > 0 {
        (idx + 1).min(PICKER_FOCUS_ORDER.len() - 1)
    } else {
        idx.saturating_sub(1)
    };
    PICKER_FOCUS_ORDER.get(next).copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SurfaceContextItem {
    Pin,
    Info,
    Close,
}

fn surface_context_item_id(surface: Surface, item: SurfaceContextItem) -> egui::Id {
    egui::Id::new(("vdock-surface-context", surface, item))
}

fn paint_surface_context_menu(
    _ui: &egui::Ui,
    surface: Surface,
    resp: &egui::Response,
    active: &mut Surface,
    pinned: &mut bool,
) {
    let mut action = None;
    resp.context_menu(|ui| {
        if context_menu_row(
            ui,
            surface_context_item_id(surface, SurfaceContextItem::Pin),
            if *pinned {
                "Unpin from rail"
            } else {
                "Pin to rail"
            },
        ) {
            action = Some(SurfaceContextItem::Pin);
            ui.close_menu();
        }
        if context_menu_row(
            ui,
            surface_context_item_id(surface, SurfaceContextItem::Info),
            "Info",
        ) {
            action = Some(SurfaceContextItem::Info);
            ui.close_menu();
        }
        if surface_closable(surface)
            && context_menu_row(
                ui,
                surface_context_item_id(surface, SurfaceContextItem::Close),
                "Close",
            )
        {
            action = Some(SurfaceContextItem::Close);
            ui.close_menu();
        }
    });

    match action {
        Some(SurfaceContextItem::Pin) => {
            *pinned = !*pinned;
        }
        Some(SurfaceContextItem::Info) => {
            *active = Surface::About;
        }
        Some(SurfaceContextItem::Close) => {
            if *active == surface {
                *active = Surface::Workbench;
            }
        }
        None => {}
    }
}

fn context_menu_row(ui: &mut egui::Ui, id: egui::Id, label: &str) -> bool {
    let width = ui.available_width().max(Style::SP_XL * 4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, Style::SP_L), egui::Sense::hover());
    let resp = ui.interact(rect, id, egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    paint_focus_ring(ui.painter(), rect, resp.has_focus());
    // WIN7-7, lock #14 — shared by both the (out-of-scope) app picker's own
    // context menu and the taskbar's Desktop cell's context menu
    // (`paint_surface_context_menu`'s two call sites); fixing it here covers
    // the taskbar-reachable one without needing a picker-specific change.
    install_cell_accessibility(ui.ctx(), id, label, "", rect);
    response_activated(ui, &resp)
}

fn rail_more_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    hidden_count: usize,
) -> bool {
    let resp = ui.interact(rect, rail_more_id(), egui::Sense::click());
    let active = state.rail_more_open || resp.hovered();
    if active {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "⋯",
        egui::FontId::proportional(Style::BODY),
        if active { Style::TEXT } else { Style::TEXT_DIM },
    );
    paint_focus_ring(ui.painter(), rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        rail_more_id(),
        "More sessions",
        format!(
            "{hidden_count} more session{}",
            if hidden_count == 1 { "" } else { "s" }
        ),
        rect,
    );
    if response_activated(ui, &resp) {
        state.rail_more_open = !state.rail_more_open;
        return true;
    }
    false
}

fn rail_more_popup(
    ui: &egui::Ui,
    anchor: egui::Rect,
    hidden_start: usize,
    sessions: &[SessionRailEntry],
    state: &mut DockState,
    opened: bool,
    rail_h: f32,
) -> bool {
    let hidden = &sessions[hidden_start..];
    let rows = hidden.len().min(8);
    let popup_w = hidden
        .iter()
        .take(rows)
        .map(|entry| session_entry_width(ui, entry, rail_h))
        .fold(rail_h * 4.0, f32::max);
    let popup_h = rows as f32 * rail_h + Style::SP_S;
    let inner = egui::Area::new(rail_more_popup_id())
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(anchor.left(), anchor.top() - Style::SP_XS))
        .show(ui.ctx(), |ui| {
            let (area, _) =
                ui.allocate_exact_size(egui::vec2(popup_w, popup_h), egui::Sense::hover());
            let bg = ui.painter().add(egui::Shape::Noop);
            let mut routed = false;
            let mut focused_session = None;
            let mut y = area.top() + Style::SP_XS / 2.0;
            for (offset, entry) in hidden.iter().take(rows).enumerate() {
                let idx = hidden_start + offset;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(area.left() + Style::SP_XS / 2.0, y),
                    egui::vec2(popup_w - Style::SP_XS, rail_h),
                )
                .shrink(2.0);
                if session_entry(ui, rect, idx, entry, state.active == Surface::Desktop) {
                    state.active = Surface::Desktop;
                    focused_session.clone_from(&entry.id);
                    routed = true;
                }
                y += rail_h;
            }
            if focused_session.is_some() {
                state.desktop_session_focus = focused_session;
            }
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(area.expand(Style::SP_XS), Style::RADIUS, Style::SURFACE),
            );
            ui.painter().rect_stroke(
                area.expand(Style::SP_XS),
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            routed
        });
    let routed = inner.inner;
    if routed {
        state.rail_more_open = false;
        true
    } else {
        if !opened && inner.response.clicked_elsewhere() {
            state.rail_more_open = false;
            return true;
        }
        false
    }
}

fn surface_closable(surface: Surface) -> bool {
    !matches!(
        surface,
        Surface::Workbench | Surface::System | Surface::Timers
    )
}

fn transfer_badge_id(surface: Surface) -> egui::Id {
    surface_badge_id(surface)
}

fn surface_badge_id(surface: Surface) -> egui::Id {
    egui::Id::new(("vdock-surface-badge", surface))
}

fn badge_label(count: usize) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

fn paint_badge(ui: &egui::Ui, cell: egui::Rect, surface: Surface, badge: BadgeKind) {
    match badge {
        BadgeKind::Count(count) => paint_count_badge(ui, cell, surface, count),
        BadgeKind::Health(tone) => paint_health_badge(ui, cell, surface, tone),
    }
}

fn paint_count_badge(ui: &egui::Ui, cell: egui::Rect, surface: Surface, count: usize) {
    let label = badge_label(count);
    let font = egui::FontId::proportional((Style::SMALL - 1.0).max(8.0));
    let galley = ui.fonts(|f| f.layout_no_wrap(label, font, egui::Color32::WHITE));
    let badge_size = egui::vec2(
        (galley.rect.width() + Style::SP_XS).max(Style::SP_M),
        Style::SP_M,
    );
    let rect = badge_rect(cell, badge_size);
    ui.interact(rect, surface_badge_id(surface), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, badge_size.y / 2.0, Style::ACCENT);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.rect.width() / 2.0,
            rect.center().y - galley.rect.height() / 2.0,
        ),
        galley,
        egui::Color32::WHITE,
    );
}

fn paint_health_badge(ui: &egui::Ui, cell: egui::Rect, surface: Surface, tone: BadgeTone) {
    let size = egui::vec2(Style::SP_S, Style::SP_S);
    let rect = badge_rect(cell, size);
    ui.interact(rect, surface_badge_id(surface), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), rect.width() / 2.0, badge_tone_color(tone));
}

fn badge_rect(cell: egui::Rect, size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(cell.right() - size.x - 2.0, cell.top() + 2.0),
        size,
    )
}

fn badge_tone_color(tone: BadgeTone) -> egui::Color32 {
    match tone {
        BadgeTone::Healthy => Style::SUPPORT_SUCCESS,
        BadgeTone::Degraded => Style::SUPPORT_WARNING,
        BadgeTone::Offline => Style::SUPPORT_ERROR,
    }
}

/// Whether `resp` should activate its target this frame: a click, or
/// Enter/Space while it holds keyboard focus — the picker cell's activation
/// contract. `pub(crate)` (not private) because WIN7-3's Start Menu tile
/// grid (`start_menu.rs`) reuses this SAME predicate for its own tiles
/// rather than reimplementing click-vs-keyboard activation a second time.
pub(crate) fn response_activated(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.clicked()
        || (resp.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
}

/// Keyboard-focus-ring stroke width (a11y-03 / WCAG 2.4.7). Matches the shell's
/// established focus-ring width (`explorer.rs`/`console.rs` `FOCUS_RING_W = 2.5`)
/// so every focus indicator across the shell reads at one consistent weight
/// against the Quasar-dark ground on the raw-painted cells.
const FOCUS_RING_W: f32 = 2.5;

/// The rect a focusable cell's keyboard-focus ring strokes when `focused`, or
/// `None` when the cell does not hold focus. Inset by half the stroke so a
/// [`FOCUS_RING_W`]-wide ring sits fully INSIDE `cell` and never bleeds into a
/// neighbouring cell. Pure geometry/decision seam — unit-testable without a live
/// painter (the a11y-03 regression guard: rings the focused cell, NOT the rest).
fn focus_ring_rect(cell: egui::Rect, focused: bool) -> Option<egui::Rect> {
    focused.then(|| cell.shrink(FOCUS_RING_W / 2.0))
}

/// Paint the shared **keyboard-focus ring** on a raw-painted dock/taskbar/picker
/// cell (a11y-03, WCAG 2.4.7 *Focus Visible*). Every focusable cell in this module
/// is a hand-rolled `ui.interact(rect, id, Sense::click())` widget, so egui emits
/// no default focus visual for it — Enter/Space/arrow navigation already works but
/// the focused cell was invisible. This is the ONE focus indicator every focusable
/// cell shares: when the cell holds keyboard focus, a high-contrast accent stroke
/// around its edge shows a keyboard user which cell is focused.
///
/// The Quasar palette has no dedicated focus token, so the ring wears the lifted
/// brand accent [`Style::ACCENT_HI`] — the same accent egui's own `selection.stroke`
/// derives its focus/selection ring from (`mde_egui::Style::accent_visuals`), one
/// rung brighter than the resting [`Style::ACCENT`] so it stays legible over the
/// selection wash a selected cell already wears.
fn paint_focus_ring(painter: &egui::Painter, cell: egui::Rect, focused: bool) {
    if let Some(ring) = focus_ring_rect(cell, focused) {
        painter.rect_stroke(
            ring,
            Style::RADIUS,
            egui::Stroke::new(FOCUS_RING_W, Style::ACCENT_HI),
            egui::StrokeKind::Inside,
        );
    }
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
    status: &StatusInputs,
    transfer_active_count: usize,
    active: &mut Surface,
    pinned: &mut bool,
) -> (f32, bool) {
    let painter = ui.painter().clone();

    // The horizontal accent label above the group (#4) — display-only (hover sense)
    // so the harness can read its rect; painted in the group accent, centred.
    let label_rect = egui::Rect::from_min_size(origin, egui::vec2(width, PICK_LABEL_H));
    ui.interact(label_rect, pick_label_id(group.label), egui::Sense::hover());
    let galley = ui.fonts(|f| f.layout_no_wrap(group.label.to_owned(), font.clone(), group.accent));
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
        let badge = badge_for(surface, status, transfer_active_count);
        if pick_app_cell_with_badge(ui, surface, active, pinned, cell, badge) {
            routed = true;
        }
    }
    let cells_bottom = cells_top + group.surfaces.len() as f32 * APP_CELL_H;

    // A FULL 1px outline in the group's accent around the whole cell cluster
    // (operator directive: each colored box gets a complete outside outline, all
    // four sides — replacing the old half-enclosure left-rail stripe + bottom
    // divider). Inset SP_XS from the column edges so the box reads as its own
    // fully-enclosed group. Every colour is a Style token (§4).
    let box_rect = egui::Rect::from_min_max(
        egui::pos2(origin.x + Style::SP_XS, cells_top),
        egui::pos2(origin.x + width - Style::SP_XS, cells_bottom),
    );
    painter.rect_stroke(
        box_rect,
        Style::RADIUS,
        egui::Stroke::new(HAIRLINE_W, group.accent),
        egui::StrokeKind::Inside,
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
        f.layout_no_wrap(
            "…".to_owned(),
            egui::FontId::proportional(Style::BODY),
            color,
        )
    });
    ui.painter().galley(
        egui::pos2(
            more.center().x - dots.size().x / 2.0,
            more.center().y - dots.size().y / 2.0,
        ),
        dots,
        color,
    );

    paint_focus_ring(ui.painter(), more, resp.has_focus());

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
                let (h, r) = pick_group(
                    ui,
                    group,
                    egui::pos2(area.left(), y),
                    DOCK_W,
                    font,
                    &state.status,
                    state.transfer_active_count,
                    &mut state.active,
                    &mut state.pinned,
                );
                y += h;
                routed |= r;
            }
            let panel = area.expand(Style::SP_S);
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(panel, Style::RADIUS, Style::SURFACE),
            );
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

// ═══════════════════════════════════════════════════════════════════════════
// NODE-GRADE-2 — the **grade mini-list** (design `docs/design/node-grade.md`,
// locks #4/#5/#6/#7/#8/#14/#15/#18/#19). A stacked A–F capability grade per mesh
// node, painted in the dock's bottom zone ABOVE the NOTIF-3 status strip (a new
// band between the app groups and the quads). The local node is pinned first with a
// "you are here" marker (#18); peers sort worst-grade-first (#19). Each row is the
// A–F letter in the shared green→red `mde_egui` ramp (#4) — hard-blinking for a D/F
// alarm (#6/#16) — a tiny load bar for the 0–100 score (#5), and a ↑/→/↓ trend
// arrow (#14). A tap opens that node's Explorer hero (#7); the worst-N show inline
// with a '…' expander for the rest (#8). No header (#15); a stale/absent grade reads
// a greyed "?" (§7). The fold + sort live in `chrome::NodeGrades`; this is the render.
// ═══════════════════════════════════════════════════════════════════════════

/// One grade mini-list row's height (design #5) — compact, on the 8px grid, tall
/// enough to seat the ~18px grade-letter cell in the quad idiom. `SP_L` (24px).
const GRADE_ROW_H: f32 = Style::SP_L;

/// The worst-N grade rows shown inline before the rest fold into the '…' expander
/// (#8) — the local node's pin plus the worst peers, bounded so the band never eats
/// the narrow column on a busy mesh.
const GRADE_MAX_ROWS: usize = 5;

/// The grade letter's cell edge — the ~18px quad idiom (#5), matching the status
/// quads' glyph edge so the three bottom clusters read on one grid.
const GRADE_LETTER_W: f32 = SYS_QUAD_ICON;

/// The trend-arrow cell width (#14) — a slim `SP_M` column at the row's right edge.
const GRADE_ARROW_W: f32 = Style::SP_M;

/// The grade load bar's height (#5) — a thin `SP_XS` rule, vertically centred.
const GRADE_BAR_H: f32 = Style::SP_XS;

/// The stable per-host id of a grade row, so the render + routing are addressable —
/// tests read a row's settled `Rect` back to click it (the [`pick_cell_id`] idiom,
/// kept distinct so a grade row never shares an id with a picker / quad cell).
fn grade_row_id(host: &str) -> egui::Id {
    egui::Id::new(("vdock-grade-row", host))
}

/// The stable id of the grade list's '…' expander cell (#8).
fn grade_overflow_id() -> egui::Id {
    egui::Id::new("vdock-grade-overflow")
}

/// The vertical space the grade mini-list claims above the status strip: the visible
/// rows (capped at [`GRADE_MAX_ROWS`], #8) each [`GRADE_ROW_H`] tall, plus the '…'
/// expander cell + a top separator gap when peers spill past the cap. `0` when there
/// are no grades (pre-poll / empty), so the band vanishes and the app zone reclaims
/// the space (the layout is then byte-identical to the pre-NODE-GRADE dock).
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)] // tiny row count; the band arithmetic reads clearer than mul_add
fn grade_band_height(grades: &NodeGrades) -> f32 {
    let total = grades.rows.len();
    if total == 0 {
        return 0.0;
    }
    let visible = total.min(GRADE_MAX_ROWS);
    let overflow = if total > GRADE_MAX_ROWS {
        OVERFLOW_H
    } else {
        0.0
    };
    GROUP_DIVIDER_H + visible as f32 * GRADE_ROW_H + overflow
}

/// Paint the grade mini-list band into the column between `grade_top` and the status
/// quads. A BORDER hairline sets it apart from the app groups above (the pin-strip
/// separator idiom) — no header (#15). Renders the worst-N rows inline, then the '…'
/// expander when peers overflow (#8). Returns `true` if a row (or a popup row) tapped
/// this frame (the caller records the tap-to-hero route, #7).
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)] // tiny row indices; layout math reads clearer than mul_add
fn paint_grade_band(
    ui: &egui::Ui,
    rect: egui::Rect,
    grade_top: f32,
    state: &mut DockState,
) -> bool {
    let total = state.status.grades.rows.len();
    let visible = total.min(GRADE_MAX_ROWS);
    let has_overflow = total > GRADE_MAX_ROWS;

    // The separating hairline (the pin-strip / system-quad rule idiom, §4 token).
    ui.painter().hline(
        (rect.left() + Style::SP_XS)..=(rect.right() - Style::SP_XS),
        grade_top + GROUP_DIVIDER_H / 2.0,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );

    // The visible rows. The tap target is collected as an owned host so the immutable
    // borrow of `state.status.grades` releases before `request_node_focus` writes.
    let rows_top = grade_top + GROUP_DIVIDER_H;
    let mut tapped: Option<String> = None;
    for (i, row) in state.status.grades.rows.iter().take(visible).enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rows_top + i as f32 * GRADE_ROW_H),
            egui::vec2(DOCK_W, GRADE_ROW_H),
        );
        if grade_row(ui, row, cell) {
            tapped = Some(row.host.clone());
        }
    }
    let mut clicked = tapped.is_some_and(|host| {
        state.request_node_focus(&host);
        true
    });

    if has_overflow {
        let more_top = rows_top + visible as f32 * GRADE_ROW_H;
        if grade_overflow(ui, rect, more_top, visible, state) {
            clicked = true;
        }
    }
    clicked
}

/// One grade mini-list row (design #4/#5/#14/#18): the local "you are here" marker
/// (#18), the A–F letter in its green→red band colour (#4) — hard-blinking on/off for
/// a D/F alarm (#6/#16) — a tiny load bar for the 0–100 score (#5), and the trend
/// arrow (#14). A stale/unobservable node reads a greyed "?" (#17/§7), never a fake
/// letter. A hover fills only (no tooltip). A click returns `true` (the caller records
/// the tap-to-hero route, #7). Every colour is an `mde_egui` token (§4).
#[allow(clippy::suboptimal_flops)] // the glyph-centring math reads clearer than mul_add
fn grade_row(ui: &egui::Ui, row: &GradeRow, cell: egui::Rect) -> bool {
    let resp = ui.interact(cell, grade_row_id(&row.host), egui::Sense::click());
    let painter = ui.painter().clone();
    if resp.hovered() {
        painter.rect_filled(cell, Style::RADIUS, Style::SURFACE_HI);
    }
    // The local node's subtle "you are here" left-edge accent tick (#18) — the
    // picker's active-bar idiom at the row's outer edge.
    if row.is_local {
        let bar =
            egui::Rect::from_min_size(cell.left_top(), egui::vec2(ACTIVE_BAR_W, cell.height()));
        painter.rect_filled(bar, egui::CornerRadius::ZERO, Style::ACCENT);
    }

    // The band the score falls into — the ONE authority for the letter, its ramp
    // colour, and whether it alarms (`mde_egui::GradeBand`, §4). A stale row never
    // alarms (we can't observe it; it reads "?").
    let band = GradeBand::from_score(f32::from(row.score));
    let alarm = !row.stale && band.is_alert();
    // A D/F alarm hard-blinks; when dark (or stale) the mark reads dim (#6/#16
    // always-blink, reduce-motion ignored).
    let lit = !alarm || Motion::blink(ui.ctx());

    // ── the A–F letter (or a greyed "?" when stale) in the ~18px quad cell ──
    let letter_rect = egui::Rect::from_min_size(
        egui::pos2(cell.left() + Style::SP_XS, cell.top()),
        egui::vec2(GRADE_LETTER_W, cell.height()),
    );
    let (glyph, letter_color) = if row.stale {
        ("?".to_owned(), Style::TEXT_DIM)
    } else if lit {
        (band.letter().to_string(), band.color())
    } else {
        (band.letter().to_string(), Style::TEXT_DIM)
    };
    let galley = ui
        .fonts(|f| f.layout_no_wrap(glyph, egui::FontId::proportional(Style::BODY), letter_color));
    painter.galley(
        egui::pos2(
            letter_rect.center().x - galley.size().x / 2.0,
            letter_rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        letter_color,
    );

    // ── the tiny load bar (the 0–100 score) ──
    let bar_left = letter_rect.right() + Style::SP_XS;
    let bar_right = cell.right() - GRADE_ARROW_W - Style::SP_XS;
    let bar_track = egui::Rect::from_min_max(
        egui::pos2(bar_left, cell.center().y - GRADE_BAR_H / 2.0),
        egui::pos2(bar_right, cell.center().y + GRADE_BAR_H / 2.0),
    );
    painter.rect_filled(bar_track, Style::RADIUS, Style::SURFACE_HI);
    if !row.stale && bar_track.width() > 0.0 {
        let fill_w = bar_track.width() * (f32::from(row.score) / 100.0).clamp(0.0, 1.0);
        let fill = egui::Rect::from_min_size(bar_track.min, egui::vec2(fill_w, bar_track.height()));
        // The load bar rides the SAME green→red ramp as the letter, dimming in
        // lock-step with the alarm blink so the whole row flashes as one (#5/#6).
        let fill_color = if lit {
            Style::grade_fill(f32::from(row.score))
        } else {
            Style::TEXT_DIM
        };
        painter.rect_filled(fill, Style::RADIUS, fill_color);
    }

    // ── the trend arrow (#14) ──
    let arrow_rect = egui::Rect::from_min_size(
        egui::pos2(cell.right() - GRADE_ARROW_W, cell.top()),
        egui::vec2(GRADE_ARROW_W, cell.height()),
    );
    let arrow = ui.fonts(|f| {
        f.layout_no_wrap(
            row.trend.arrow().to_owned(),
            egui::FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        )
    });
    painter.galley(
        egui::pos2(
            arrow_rect.center().x - arrow.size().x / 2.0,
            arrow_rect.center().y - arrow.size().y / 2.0,
        ),
        arrow,
        Style::TEXT_DIM,
    );

    paint_focus_ring(&painter, cell, resp.has_focus());
    resp.clicked()
}

/// The grade list's '…' **expander** (design #8) — when peers spill past
/// [`GRADE_MAX_ROWS`], a '…' cell beneath the visible rows toggles a floating popup
/// of the hidden (better-graded) peers, each still tapping through to its hero.
/// Reuses the app picker's overflow idiom ([`pick_overflow`]): a SURFACE panel +
/// hairline behind the same rows, anchored to the right and growing upward. Returns
/// `true` when a popup row tapped this frame.
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)] // tiny row count; layout math reads clearer than mul_add
fn grade_overflow(
    ui: &egui::Ui,
    rect: egui::Rect,
    more_top: f32,
    visible: usize,
    state: &mut DockState,
) -> bool {
    let more = egui::Rect::from_min_size(
        egui::pos2(rect.left(), more_top),
        egui::vec2(DOCK_W, OVERFLOW_H),
    );
    let resp = ui.interact(more, grade_overflow_id(), egui::Sense::click());
    let opened = resp.clicked();
    if opened {
        state.grades_overflow_open = !state.grades_overflow_open;
    }
    let color = if state.grades_overflow_open || resp.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let dots = ui.fonts(|f| {
        f.layout_no_wrap(
            "…".to_owned(),
            egui::FontId::proportional(Style::BODY),
            color,
        )
    });
    ui.painter().galley(
        egui::pos2(
            more.center().x - dots.size().x / 2.0,
            more.center().y - dots.size().y / 2.0,
        ),
        dots,
        color,
    );

    paint_focus_ring(ui.painter(), more, resp.has_focus());

    if !state.grades_overflow_open {
        return false;
    }

    // The hidden peer rows (past the worst-N cut) — cloned so the immutable grades
    // borrow releases before `request_node_focus` writes state.
    let hidden: Vec<GradeRow> = state
        .status
        .grades
        .rows
        .iter()
        .skip(visible)
        .cloned()
        .collect();
    let popup_h = hidden.len() as f32 * GRADE_ROW_H;
    let mut tapped: Option<String> = None;
    let inner = egui::Area::new(egui::Id::new("vdock-grade-overflow-popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(more.right() + Style::SP_XS, more.bottom()))
        .show(ui.ctx(), |ui| {
            let (area, _) =
                ui.allocate_exact_size(egui::vec2(DOCK_W, popup_h), egui::Sense::hover());
            // Reserve a slot so the panel background paints BEHIND the rows.
            let bg = ui.painter().add(egui::Shape::Noop);
            for (i, row) in hidden.iter().enumerate() {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(area.left(), area.top() + i as f32 * GRADE_ROW_H),
                    egui::vec2(DOCK_W, GRADE_ROW_H),
                );
                if grade_row(ui, row, cell) {
                    tapped = Some(row.host.clone());
                }
            }
            let panel = area.expand(Style::SP_S);
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(panel, Style::RADIUS, Style::SURFACE),
            );
            ui.painter().rect_stroke(
                panel,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
        });

    if let Some(host) = tapped {
        // A tap routes to the hero + closes the popup (the tray idiom).
        state.request_node_focus(&host);
        state.grades_overflow_open = false;
        return true;
    }
    if !opened && inner.response.clicked_elsewhere() {
        // Click-away dismissal — but not on the very click that opened it.
        state.grades_overflow_open = false;
    }
    false
}

mod system_quad;
use system_quad::*;

#[cfg(test)]
mod tests;
