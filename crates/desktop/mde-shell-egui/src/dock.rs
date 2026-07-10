//! The shell **dock** — the left **vertical dock** ([`dock`], design
//! `docs/design/vertical-dock.md`): the shell's ONE chrome (VDOCK, the sole
//! surface launcher after VDOCK-6b ripped out the old horizontal taskbar).
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
//! no tooltips anywhere. Beneath the picker sit VDOCK-5's **clock strip** (the
//! live HH:MM glyph that opens Timers & Alarms, lock #20) and VDOCK-4's **system
//! quad** (Settings · Show-Desktop · Lock · Power). Notification status pips live
//! in the separate bottom rail, not in the left rail.
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
    pub(crate) const ALL: [Surface; 17] = [
        Surface::Workbench,
        Surface::MeshView,
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
/// all 17 of [`Surface::ALL`]. Drives the picker render + the shell tests (the one
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

const PICKER_FOCUS_ORDER: [Surface; 16] = [
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
// The shell's sole chrome (VDOCK-6b removed the old horizontal taskbar): a
// left-edge, full-height, ~48px, solid Carbon-dark column that slides in from the
// left and auto-hides (hotkey + pin, no hover). VDOCK-1 builds the FRAME + the
// slide/toggle/pin mechanism; the interior is filled by the app picker (VDOCK-2),
// the clock strip (VDOCK-5), and the system quad (VDOCK-4). Notification pips
// mount in the separate bottom rail.
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
    /// CONSOLE-1 — whether the Console front-door panel is up, mirrored in by
    /// the shell each frame ([`Self::set_console_open`]) so the Start cell wears
    /// its active tint (the `set_active` mirror idiom).
    console_open: bool,
    /// CONSOLE-1 — latched `true` by a Start-cell click ([`start_cell`]); the
    /// shell drains it ([`Self::take_console_toggle`]) and toggles the Console
    /// panel. The dock can't reach the panel itself (§6, the deferred wire).
    console_toggle: bool,
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

    const fn rail_height(&self) -> f32 {
        match self.density {
            Density::Mouse => NOTIFICATION_RAIL_H,
            Density::Touch => NOTIFICATION_RAIL_EXPANDED_H,
        }
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

    /// Mirror the Console front-door panel's open state into the dock before
    /// [`dock`] (CONSOLE-1) — the Start cell's active tint then follows the real
    /// panel (the [`Self::set_active`] mirror idiom). Wired by
    /// `main.rs::mount_dock_chrome`.
    pub const fn set_console_open(&mut self, open: bool) {
        self.console_open = open;
    }

    /// Drain the Start cell's **Console toggle** (CONSOLE-1) — `true` exactly
    /// once per Start-cell click; the shell flips the Console panel on it (the
    /// [`Self::take_request`] deferred-wire idiom). Pressing Start with the
    /// panel already up drains through the same latch and closes it (lock #4).
    pub const fn take_console_toggle(&mut self) -> bool {
        let toggled = self.console_toggle;
        self.console_toggle = false;
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
/// VDOCK-1 built the FRAME + the slide/toggle/pin; **VDOCK-2** fills the top
/// **Workbench-lead** zone + the single-column **app-groups** middle; **VDOCK-3**
/// fills the bottom **status strip** and **VDOCK-4** the **system quad** beneath
/// them ([`paint_dock_frame`]). Returns `true` if a dock control routed this frame
/// — the pin, a picker cell, a status segment selecting its [`Surface`], or a
/// system-quad cell (a route, the curtain lock, or the Power menu), recorded in
/// [`DockState`] (the active surface + the pending lock/power requests) which the
/// shell reads back to surface the body / drive the seat.
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

/// Render the bottom rail that holds the small global controls removed from the
/// left rail: Advanced menu, Desktop, Clock, Pin, and the notification/status
/// segment micro-icons.
#[cfg(test)]
pub fn notification_rail(ctx: &egui::Context, state: &mut DockState) -> bool {
    notification_rail_with_sources(ctx, state, &[])
}

/// Render the bottom rail with the compact Desktop source flyout fed from
/// `ChooserState` by the shell.
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
            let local = egui::Rect::from_min_size(
                egui::pos2(0.0, rail_rect.top() - area_top),
                rail_rect.size(),
            );
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

            let tray_icon_w = rail_h.min(NOTIFICATION_RAIL_EXPANDED_ICON_H) - 4.0;
            let status_w = tray_icon_w * status::StatusSegment::ALL.len() as f32;
            let clock_w = rail_h * 2.2;
            let right_cluster_w =
                clock_w + rail_h + status_w + Style::SP_XS + rail_h + Style::SP_XS;
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
                    let opened_more = rail_more_cell(ui, more, state);
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
            let mut tray_x = local.right() - Style::SP_XS - clock_w;
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
            tray_x -= rail_h;
            if pin_toggle(ui, cell(tray_x), state) {
                clicked = true;
            }
            tray_x -= status_w + Style::SP_XS;

            let status_rect = egui::Rect::from_min_size(
                egui::pos2(tray_x, local.top() + 2.0),
                egui::vec2(
                    status_w,
                    rail_h.min(NOTIFICATION_RAIL_EXPANDED_ICON_H) - 4.0,
                ),
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

/// Paint the vertical dock's frame into `rect` and lay out its interior: the solid
/// Carbon-dark panel + the hairline right-edge divider (lock #24, §4 tokens), the
/// **VDOCK-2** top zone (the Workbench lead + the folded-in pin) and middle zone
/// (the single-column app groups + '…' overflow), VDOCK-5's clock strip, and the
/// **VDOCK-4** system quad in the final `DOCK_W` row beneath it. Returns `true`
/// if the pin, a picker cell, the clock, or a system-quad cell routed/acted this
/// frame.
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
    // Advanced menu, Desktop shortcut, clock, and pin live in the bottom rail.
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

/// The stable id of CONSOLE-1's Start cell, so tests read its settled `Rect`.
fn start_cell_id() -> egui::Id {
    egui::Id::new("vdock-start-cell")
}

fn rail_icon(ui: &egui::Ui, rect: egui::Rect, icon: IconId, tint: egui::Color32) -> bool {
    let resp = ui.interact(
        rect,
        egui::Id::new(("bottom-rail-icon", icon.name())),
        egui::Sense::click(),
    );
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
    apply_picker_arrow_focus(ui, surface, &resp);
    paint_surface_context_menu(ui, surface, &resp, active, pinned);
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
    if resp.clicked() {
        state.status_panel_open = !state.status_panel_open;
        return true;
    }
    false
}

/// CONSOLE-1's **Start cell** — the Console front door's trigger (console
/// design locks #1/#2): the bottom rail's far-left Advanced affordance, wearing
/// the repo's Win10-style Start/Menu tray glyph. A click latches the
/// Console toggle for the shell to drain ([`DockState::take_console_toggle`] —
/// the deferred wire; pressing it again closes, lock #4). While the panel is up
/// (mirrored in via [`DockState::set_console_open`]) the cell wears the
/// selection wash + ACCENT tint, the sys-cell "menu open" idiom. Every colour
/// is a Style token (§4). Returns `true` on a click.
fn start_cell(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, start_cell_id(), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if state.console_open {
        painter.rect_filled(rect, Style::RADIUS, ui.visuals().selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if state.console_open {
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
    paint_rail_label(ui, rect, "Advanced", tint);
    if resp.clicked() {
        state.console_toggle = true;
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
    painter.text(
        if rect.height() >= NOTIFICATION_RAIL_EXPANDED_H - 1.0 {
            egui::pos2(rect.center().x, rect.top() + Style::SP_M)
        } else {
            rect.center()
        },
        egui::Align2::CENTER_CENTER,
        crate::timers::hhmm(now),
        egui::FontId::proportional(Style::SMALL.min((rect.height() - 6.0).max(8.0))),
        tint,
    );
    paint_rail_label(ui, rect, "Clock", tint);
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
/// remains in the left rail. Advanced, Desktop, Clock, Pin, and notification
/// status micro-icons live in the full-width bottom rail.
const BOTTOM_ZONE_H: f32 = DOCK_W;

/// The full-width bottom rail height. It is intentionally thinner than a dock
/// cell; controls render at micro-icon scale inside it.
const NOTIFICATION_RAIL_H: f32 = 20.0;
/// NAVBAR-8's expanded bar height: touch/expanded density grows the rail to a
/// labelled Win10-style taskbar variant while compact density keeps the 20px rail.
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
            mde_cosmic_applet::LighthouseHealth::AllHealthy => BadgeTone::Healthy,
            mde_cosmic_applet::LighthouseHealth::Degraded => BadgeTone::Degraded,
            mde_cosmic_applet::LighthouseHealth::None => BadgeTone::Offline,
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
    if let Some(badge) = badge {
        paint_badge(ui, rect, surface, badge);
    }
    apply_picker_arrow_focus(ui, surface, &resp);
    paint_surface_context_menu(ui, surface, &resp, active, pinned);

    if response_activated(ui, &resp) {
        *active = surface;
        return true;
    }
    false
}

fn apply_picker_arrow_focus(ui: &egui::Ui, surface: Surface, resp: &egui::Response) {
    if !resp.has_focus() {
        return;
    }
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
    response_activated(ui, &resp)
}

fn rail_more_cell(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
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

fn response_activated(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.clicked()
        || (resp.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
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

// ═══════════════════════════════════════════════════════════════════════════
// VDOCK-4 — the **system quad** + Power menu (design `docs/design/vertical-dock.md`,
// locks #7/#17/#18). The final DOCK_W row of the bottom band holds a 2×2 control
// cluster sized to match the compact dock cells: Settings · Show-Desktop · Lock ·
// Power (#7/#17). Settings routes to `Surface::System`, Show-Desktop to the existing
// `Surface::Desktop` route (#15's control analogue), Lock drops the shell curtain
// (the same in-process lock Super+L / the idle honorer trigger), and Power opens the
// armed Lock/Suspend/Reboot/Shutdown menu (#18) — Reboot + Shutdown demand a typed
// echo before they fire (the storage surface's typed-arming idiom, lock 8's spirit).
// Every verb drives the REAL seam: Lock → `curtain.lock()`, Suspend/Reboot/Shutdown →
// `system.honor_power` (§6 — never a raw `systemctl`), both drained by the shell from
// `DockState` (the deferred `main.rs` wire, out of this dock.rs-only fence).
// ═══════════════════════════════════════════════════════════════════════════

/// The system-quad glyph edge — ~18px (design #12/#23), restated on the shared
/// 8px grid (`SP_M` + half an `SP_XS`). The `SYS_QUAD_ICON` test pins it smaller
/// than the 24px app glyph (#12).
const SYS_QUAD_ICON: f32 = Style::SP_M + Style::SP_XS / 2.0;

/// The stroke width of the procedurally-drawn system-quad glyphs (Lock + Power —
/// the brand set has no glyph for either yet, like the VDOCK-1 pin): a 2px rule
/// (`HAIRLINE_W · 2`), so the line-art reads at the ~18px quad-icon size.
const SYS_GLYPH_STROKE: f32 = HAIRLINE_W * 2.0;

/// The Power menu's row + popup width — token math (`SP_XL · 5` = 160pt), wide
/// enough for the host-down confirm buttons and typed-arming field on one line.
const POWER_MENU_W: f32 = Style::SP_XL * 5.0;

/// One Power-menu row's height — compact, on the 8px grid (`SP_L`).
const POWER_ROW_H: f32 = Style::SP_L;

/// One cell of the 2×2 **system quad** (design #7/#17), row-major.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SysCell {
    /// The host-controls **Settings** cell — routes to [`Surface::System`].
    Settings,
    /// The Win10 **Show-Desktop** cell — the existing [`Surface::Desktop`] route.
    ShowDesktop,
    /// The **Lock** cell — drops the shell curtain (records a lock request).
    Lock,
    /// The **Power** cell — toggles the armed Lock/Suspend/Reboot/Shutdown menu (#18).
    Power,
}

impl SysCell {
    /// The brand glyph for the cell, or `None` for the procedurally-drawn Lock +
    /// Power (the brand set has no glyph for either yet — the VDOCK-1 pin precedent).
    const fn glyph(self) -> Option<IconId> {
        match self {
            Self::Settings => Some(IconId::Settings),
            Self::ShowDesktop => Some(IconId::Desktop),
            Self::Lock | Self::Power => None,
        }
    }

    /// The semantic Carbon tone for the cell's action glyph. Inactive cells dim to
    /// the same baseline as status pips; hover/active reveal this tone.
    const fn tone(self) -> egui::Color32 {
        match self {
            Self::Settings => Style::SUPPORT_INFO,
            Self::ShowDesktop => Style::SUPPORT_SUCCESS,
            Self::Lock => Style::SUPPORT_WARNING,
            Self::Power => Style::SUPPORT_ERROR,
        }
    }
}

/// The four system-quad cells in row-major order (design #17) — the one authority
/// the render + routing + tests read.
const SYSTEM_QUAD: [SysCell; 4] = [
    SysCell::Settings,
    SysCell::ShowDesktop,
    SysCell::Lock,
    SysCell::Power,
];

/// One item of the Power cell's menu (design #18). `Lock` drops the curtain (NOT
/// logind's session Lock); the rest drive their real [`PowerVerb`]. Reboot +
/// Shutdown are typed-armed; Lock + Suspend act on a single click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PowerItem {
    /// Drop the shell curtain (the in-process lock).
    Lock,
    /// Suspend-to-RAM — reversible, so no typed arming (a single click acts).
    Suspend,
    /// Reboot the host — typed-armed (design #18).
    Reboot,
    /// Power the host off — typed-armed (design #18); the design's "Shutdown".
    Shutdown,
}

/// The Power menu's four items in render order (design #18).
const POWER_MENU: [PowerItem; 4] = [
    PowerItem::Lock,
    PowerItem::Suspend,
    PowerItem::Reboot,
    PowerItem::Shutdown,
];

impl PowerItem {
    /// The operator-facing label — the design #18 names ("Shutdown", not logind's
    /// "Power off"); the typed-arming echo must match this exactly.
    const fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Suspend => "Suspend",
            Self::Reboot => "Reboot",
            Self::Shutdown => "Shutdown",
        }
    }

    /// Whether this verb demands a typed-arming echo before it fires — the
    /// host-down Reboot + Shutdown (design #18); Lock + Suspend act at once.
    const fn typed_armed(self) -> bool {
        matches!(self, Self::Reboot | Self::Shutdown)
    }

    /// The real [`PowerVerb`] this item drives through the seat power seam —
    /// `None` for Lock (which drops the curtain, not a logind verb).
    const fn power_verb(self) -> Option<PowerVerb> {
        match self {
            Self::Lock => None,
            Self::Suspend => Some(PowerVerb::Suspend),
            Self::Reboot => Some(PowerVerb::Reboot),
            Self::Shutdown => Some(PowerVerb::PowerOff),
        }
    }
}

/// The Power menu's cross-frame state (VDOCK-4, design #18): whether the anchored
/// popup is open, and the host-down verb being **typed-armed** with its echo
/// buffer. Kept tiny + pure so the arming gate ([`Self::armed`]) is unit-tested
/// without a GPU.
#[derive(Debug, Default)]
struct PowerMenu {
    /// Whether the anchored popup is open (toggled by the Power cell).
    open: bool,
    /// The verb awaiting its typed confirmation (Reboot / Shutdown) + the
    /// operator-typed echo; `None` while the menu shows its top-level verb list.
    arming: Option<Arming>,
}

/// A host-down verb mid typed-arming: the verb + the echo the operator types to
/// arm it (the storage surface's arming-echo idiom).
#[derive(Debug)]
struct Arming {
    /// The verb this stage will fire once its echo matches.
    verb: PowerItem,
    /// The operator-typed echo — must equal [`PowerItem::label`] (case-insensitive)
    /// for [`PowerMenu::armed`] to be `true`.
    echo: String,
}

impl PowerMenu {
    /// Toggle the popup (the Power cell); closing it drops any in-flight arming.
    fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.arming = None;
        }
    }

    /// Close the popup + clear any arming (a fired verb, or a click-away).
    fn close(&mut self) {
        self.open = false;
        self.arming = None;
    }

    /// Enter the typed-arming stage for a host-down verb, with an empty echo.
    fn arm(&mut self, verb: PowerItem) {
        self.arming = Some(Arming {
            verb,
            echo: String::new(),
        });
    }

    /// Whether the in-flight arming's echo matches its verb's label — the gate a
    /// Reboot/Shutdown confirm must pass (§7 — a blank / mistyped echo never fires).
    fn armed(&self) -> bool {
        self.arming
            .as_ref()
            .is_some_and(|a| a.echo.trim().eq_ignore_ascii_case(a.verb.label()))
    }
}

/// The stable per-cell id of a system-quad cell, so the render + routing are
/// unchanged but the layout is addressable — tests read a cell's settled `Rect`
/// back to click its centre, kept distinct so a system cell never shares an id
/// with a status/picker cell.
fn sys_cell_id(cell: SysCell) -> egui::Id {
    egui::Id::new(("vdock-system-quad-cell", cell))
}

/// The stable id of a Power-menu row (design #18), so tests can read its rect back.
fn power_item_id(item: PowerItem) -> egui::Id {
    egui::Id::new(("vdock-power-item", item))
}

/// The Power-menu typed-arming field's stable id (the one field the stage owns).
fn power_arming_field_id() -> egui::Id {
    egui::Id::new("vdock-power-arming-field")
}

/// Render VDOCK-4's **system quad** into the dock's final `DOCK_W` row (design
/// #7/#17): a 2×2 of `quad / 2`-square cells (matching the compact dock grid),
/// `origin` at its top-left. Each cell routes/acts on a click — Settings→System,
/// Show-Desktop→Desktop, Lock→the curtain, Power→the armed menu (#18). Paints
/// through `ui.interact` over explicit rects (the dock's `&Ui` idiom), so it
/// composes inside `paint_dock_frame`. Returns `true` if a cell routed/acted.
#[allow(
    clippy::cast_precision_loss, // the 0..4 cell indices are tiny
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn system_quad(ui: &egui::Ui, state: &mut DockState, origin: egui::Pos2, quad: f32) -> bool {
    let cell = quad / 2.0;
    let mut routed = false;
    let mut power_rect = None;
    // `opened` marks the click that just opened the Power menu THIS frame, so the
    // menu's same-frame click-away check doesn't read its own opening click (which
    // lands on the cell, outside the popup) as a dismissal — the tray-flyout guard.
    let mut opened = false;
    for (i, &c) in SYSTEM_QUAD.iter().enumerate() {
        let (row, col) = (i / 2, i % 2);
        let rect = egui::Rect::from_min_size(
            egui::pos2(origin.x + col as f32 * cell, origin.y + row as f32 * cell),
            egui::vec2(cell, cell),
        );
        if c == SysCell::Power {
            power_rect = Some(rect);
        }
        if sys_cell(ui, c, state, rect) {
            route_sys_cell(c, state, &mut opened);
            routed = true;
        }
    }

    // The Power menu popup (design #18), anchored to the Power cell — rendered only
    // while open, so a closed menu floats no layer.
    if state.power.open {
        if let Some(anchor) = power_rect {
            if power_menu_popup(ui.ctx(), anchor, state, opened) {
                routed = true;
            }
        }
    }
    routed
}

/// Apply a system-quad cell's click (VDOCK-4): the route (Settings/Show-Desktop),
/// the curtain lock request (Lock), or the Power-menu toggle (Power). `opened` is
/// set `true` when this click just OPENED the Power menu (the click-away guard).
fn route_sys_cell(cell: SysCell, state: &mut DockState, opened: &mut bool) {
    match cell {
        SysCell::Settings => state.active = Surface::System,
        SysCell::ShowDesktop => state.active = Surface::Desktop,
        SysCell::Lock => state.request_lock(),
        SysCell::Power => {
            state.power.toggle();
            *opened = state.power.open;
        }
    }
}

/// One system-quad cell (NOTIF-12): a compact glyph pip matching the status-strip
/// language. Each action owns a semantic Carbon tone, inactive cells dim to the
/// same baseline as missing status rollups, and hover/active states reveal the tone
/// without changing the route/action behavior.
/// A click returns `true` (the caller routes). `&Ui` + `ui.interact` over the
/// explicit `rect`, so it paints inside the dock frame.
fn sys_cell(ui: &egui::Ui, cell: SysCell, state: &DockState, rect: egui::Rect) -> bool {
    let response = ui.interact(rect, sys_cell_id(cell), egui::Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter().clone();
    if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let active = match cell {
        SysCell::Settings => state.active == Surface::System,
        SysCell::ShowDesktop => state.active == Surface::Desktop,
        SysCell::Power => state.power.open,
        SysCell::Lock => false,
    };
    let tint = sys_cell_tint(cell, active, hovered);
    let icon_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(SYS_QUAD_ICON, SYS_QUAD_ICON));
    match cell.glyph() {
        // Settings / Show-Desktop: the real brand glyph through the shared loader.
        Some(id) => {
            if let Some(tex) = icon_texture(ui.ctx(), id, SYS_QUAD_ICON, tint) {
                egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
                    .paint_at(ui, icon_rect);
            }
        }
        // Lock / Power: procedural line-art (no brand glyph exists yet).
        None => match cell {
            SysCell::Lock => paint_lock_glyph(&painter, icon_rect, tint),
            SysCell::Power => paint_power_glyph(&painter, icon_rect, tint),
            _ => {}
        },
    }
    if cell == SysCell::Settings {
        if let Some(badge) = badge_for(Surface::System, &state.status, state.transfer_active_count)
        {
            paint_badge(ui, rect, Surface::System, badge);
        }
    }
    response.clicked()
}

fn sys_cell_tint(cell: SysCell, active: bool, hovered: bool) -> egui::Color32 {
    if active || hovered {
        cell.tone()
    } else {
        Style::TEXT_DIM
    }
}

/// Sample `segments + 1` points along a circular arc (centre `c`, radius `r`) from
/// `a0` to `a1` radians, in egui's y-down space (θ measured up from +x, so θ=0 is
/// right and θ=π/2 is straight up). Strokes the procedural Lock shackle + Power
/// ring (no brand glyph exists for either — the VDOCK-1 pin's procedural precedent).
#[allow(
    clippy::cast_precision_loss, // the segment count is tiny
    clippy::suboptimal_flops     // the trig sample reads clearer than mul_add
)]
fn arc_points(c: egui::Pos2, r: f32, a0: f32, a1: f32, segments: usize) -> Vec<egui::Pos2> {
    (0..=segments)
        .map(|i| {
            let t = a0 + (a1 - a0) * i as f32 / segments as f32;
            egui::pos2(c.x + r * t.cos(), c.y - r * t.sin())
        })
        .collect()
}

/// Paint a procedural **padlock** in `rect`, tinted with `tint` (a Style token) —
/// a stroked body rounded-rect, a top shackle arc, and a keyhole dot. The Lock
/// cell's glyph (the brand set has none yet).
#[allow(clippy::suboptimal_flops)] // glyph geometry reads clearer than mul_add
fn paint_lock_glyph(painter: &egui::Painter, rect: egui::Rect, tint: egui::Color32) {
    let stroke = egui::Stroke::new(SYS_GLYPH_STROKE, tint);
    let w = rect.width();
    // The body: a rounded rect filling the lower ~half of the icon.
    let body = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - w * 0.31),
        egui::vec2(w * 0.62, w * 0.5),
    );
    painter.rect_stroke(body, Style::RADIUS, stroke, egui::StrokeKind::Middle);
    // The shackle: an upward semicircle rising from the body's top edge.
    let shackle = arc_points(
        egui::pos2(body.center().x, body.top()),
        w * 0.22,
        0.0,
        std::f32::consts::PI,
        12,
    );
    painter.add(egui::Shape::line(shackle, stroke));
    // The keyhole.
    painter.circle_filled(body.center(), SYS_GLYPH_STROKE * 0.9, tint);
}

/// Paint the procedural **power symbol** (IEC 60417) in `rect`, tinted with `tint`
/// (a Style token) — a ring with a gap at the top and a vertical bar through it.
/// The Power cell's glyph (the brand set has none yet).
#[allow(clippy::suboptimal_flops)] // glyph geometry reads clearer than mul_add
fn paint_power_glyph(painter: &egui::Painter, rect: egui::Rect, tint: egui::Color32) {
    // The radians of gap left at the top of the ring (centred on θ = π/2).
    const GAP: f32 = 0.9;
    let stroke = egui::Stroke::new(SYS_GLYPH_STROKE, tint);
    let c = rect.center();
    let r = rect.width() * 0.3;
    // The ring, drawn the long way around (left → bottom → right) so it leaves the
    // gap at the top.
    let start = std::f32::consts::FRAC_PI_2 + GAP / 2.0;
    let end = std::f32::consts::FRAC_PI_2 - GAP / 2.0 + std::f32::consts::TAU;
    painter.add(egui::Shape::line(arc_points(c, r, start, end, 28), stroke));
    // The vertical bar down through the gap into the ring.
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - r * 1.15),
            egui::pos2(c.x, c.y - r * 0.1),
        ],
        stroke,
    );
}

/// The Power cell's anchored **menu** popup (design #18) — the Lock/Suspend/
/// Reboot/Shutdown list, or (for a host-down verb) the typed-arming stage. Floated
/// to the RIGHT of the Power cell, growing upward (the `pick_overflow` / tray-flyout
/// idiom): a SURFACE panel + hairline border behind the rows. A Lock/Suspend click
/// fires at once; a Reboot/Shutdown click enters arming, and its Confirm fires only
/// once the echo matches. `opened` guards the same-frame click-away. Returns `true`
/// when a verb fired this frame (the menu then closed).
fn power_menu_popup(
    ctx: &egui::Context,
    anchor: egui::Rect,
    state: &mut DockState,
    opened: bool,
) -> bool {
    let mut fired = false;
    let area = egui::Area::new(egui::Id::new("vdock-power-menu"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(anchor.right() + Style::SP_XS, anchor.bottom()))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, Style::SP_XS);
            // Reserve a slot so the panel background paints BEHIND the rows (the
            // pick_overflow / tray / keyboard overlay idiom).
            let bg = ui.painter().add(egui::Shape::Noop);
            if state.power.arming.is_some() {
                // The typed-arming stage for a host-down verb (Reboot / Shutdown).
                if let Some(item) = power_arming_stage(ui, &mut state.power) {
                    state.fire_power(item);
                    fired = true;
                }
            } else {
                // The top-level verb list.
                for &item in &POWER_MENU {
                    if power_row(ui, item).clicked() {
                        if item.typed_armed() {
                            state.power.arm(item);
                        } else {
                            state.fire_power(item);
                            fired = true;
                        }
                    }
                }
            }
            let panel = ui.min_rect().expand(Style::SP_S);
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
    // Click-away dismissal — but not on the very click that opened the menu, and
    // not when a verb already fired (which closed it).
    if !opened && !fired && area.response.clicked_elsewhere() {
        state.power.close();
    }
    fired
}

/// One Power-menu row (design #18): the verb label, hover fill only — no tooltip.
/// The host-down Reboot + Shutdown read in DANGER, Lock + Suspend in TEXT. Fixed
/// [`POWER_MENU_W`] so the popup reads as one column; addressable by a stable id.
fn power_row(ui: &mut egui::Ui, item: PowerItem) -> egui::Response {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(POWER_MENU_W, POWER_ROW_H), egui::Sense::hover());
    let response = ui.interact(rect, power_item_id(item), egui::Sense::click());
    let color = if item.typed_armed() {
        Style::DANGER
    } else {
        Style::TEXT
    };
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            item.label().to_owned(),
            egui::FontId::proportional(Style::SMALL),
            color,
        )
    });
    painter.galley(
        egui::pos2(
            rect.left() + Style::SP_S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response
}

/// The Power menu's **typed-arming stage** (design #18) for a host-down verb: the
/// echo field, a DANGER Confirm button **enabled only once the echo matches** (§7 —
/// the disabled button can't fire), and a Cancel back to the verb list. Returns
/// `Some(item)` on a confirmed (armed) click.
fn power_arming_stage(ui: &mut egui::Ui, power: &mut PowerMenu) -> Option<PowerItem> {
    let item = power.arming.as_ref().map(|a| a.verb)?;
    // The echo field (scoped so its `&mut` on the buffer ends before the arming
    // check + the buttons).
    {
        let echo = &mut power.arming.as_mut().expect("arming set above").echo;
        ui.add(
            egui::TextEdit::singleline(echo)
                .id(power_arming_field_id())
                .hint_text(item.label())
                .desired_width(POWER_MENU_W),
        );
    }
    let armed = power.armed();
    let mut fire = None;
    let mut cancel = false;
    ui.horizontal(|ui| {
        let confirm = egui::Button::new(
            egui::RichText::new(format!("Confirm {}", item.label()))
                .size(Style::SMALL)
                .color(Style::DANGER),
        );
        // A disabled button never reports a click, so this fires ONLY when armed.
        if ui.add_enabled(armed, confirm).clicked() {
            fire = Some(item);
        }
        if ui
            .button(egui::RichText::new("Cancel").size(Style::SMALL))
            .clicked()
        {
            cancel = true;
        }
    });
    if cancel {
        power.arming = None;
    }
    fire
}

#[cfg(test)]
mod tests {
    use super::{
        clock_cell_id, desktop_source_row_id, desktop_source_toggle_id, dock, grade_band_height,
        grade_overflow_id, grade_row_id, group_height, gutter_width, notification_rail,
        notification_rail_with_sources, overflow_more_id, pick_cell_id, power_item_id,
        rail_more_id, session_entry_id, start_cell_id, status_detail_toggle_id, surface_badge_id,
        surface_context_item_id, sys_cell_id, sys_cell_tint, transfer_badge_id,
        visible_group_count, DesktopRailSource, DockRequest, DockState, PowerItem, PowerMenu,
        SessionRailEntry, Surface, SurfaceContextItem, SysCell, CELL_W, DOCK_AREA, DOCK_W,
        GRADE_MAX_ROWS, GROUPS, ICON_LOGICAL, NOTIFICATION_RAIL_EXPANDED_H, NOTIFICATION_RAIL_H,
        POWER_MENU, SYSTEM_QUAD, SYS_QUAD_ICON,
    };
    use crate::chrome::{GradeRow, GradeTrend, MeshSummary, NodeGrades};
    use crate::status::{self, StatusSegments};
    use mde_egui::Style;
    use mde_egui::{egui, Density};
    use mde_seat::PowerVerb;
    use mde_theme::brand::icons::{icon_image, IconId};

    /// One grade row at a chosen host / score / pin / staleness (steady trend).
    fn grade(host: &str, score: u8, is_local: bool, stale: bool) -> GradeRow {
        GradeRow {
            host: host.to_owned(),
            score,
            trend: GradeTrend::Steady,
            is_local,
            stale,
        }
    }

    /// A seen grade set in the given (already-sorted) render order — the render
    /// preserves the order `chrome::NodeGrades::fold` produced.
    fn grades(rows: Vec<GradeRow>) -> NodeGrades {
        NodeGrades { rows, seen: true }
    }

    #[test]
    fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
        // Seventeen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
        // the brokered Desktop surface, the app surfaces (Music / Media — the
        // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
        // browser, BOOKMARKS-6 / Terminal — the Terminator-class terminal over a real
        // PTY, TERM-16 / Editor — the native Zed-style code editor, EDITOR-1), the
        // unified Chat surface (the ONE notification interface — the standalone
        // Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the Phones
        // hub (KDC-MESH-9 — the desktop-side paired-phone manager), the host-controls
        // System surface, the Storage surface (GParted-authentic disk mgmt, E12-21),
        // and the About surface (the platform-identity screen, QBRAND-6).
        assert_eq!(Surface::ALL.len(), 17);
        assert_eq!(Surface::ALL[0], Surface::Workbench);
        for s in [
            Surface::MeshView,
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
            // The Phones hub (KDC-MESH-9) — the desktop-side paired-phone manager.
            Surface::Phones,
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

    // --- QBRAND-7: every dock surface renders a brand::icons glyph ----------------

    #[test]
    fn every_surface_maps_to_a_named_brand_glyph() {
        // The map is 1:1 by name (Workbench→Workbench … MeshView→MeshView), and no
        // surface folds onto the blank text wordmark.
        let cases = [
            (Surface::Workbench, IconId::Workbench),
            (Surface::MeshView, IconId::MeshView),
            (Surface::InfraCode, IconId::Server),
            (Surface::Desktop, IconId::Desktop),
            (Surface::Music, IconId::Music),
            (Surface::Media, IconId::Media),
            (Surface::Files, IconId::Files),
            (Surface::Voice, IconId::Voice),
            (Surface::Browser, IconId::Browser),
            (Surface::Bookmarks, IconId::Bookmarks),
            (Surface::Terminal, IconId::Terminal),
            (Surface::Editor, IconId::Editor),
            (Surface::Chat, IconId::Chat),
            // The Phones hub wears its own smartphone glyph (KDC-MESH-9).
            (Surface::Phones, IconId::Phones),
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
        // The map is injective — 17 surfaces, 17 distinct glyph names (IaC wears
        // the Server badge, unshared by any other surface).
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

    // --- PICKER-1: the group table + rotated labels + hairline dividers -----------

    #[test]
    fn the_locked_group_taxonomy_and_order() {
        // L5/L7 — six groups in the locked left-to-right order, each listing its
        // surfaces in Surface::ALL relative order; About lives in the System group.
        // THREE surfaces are in no group: the Workbench (standalone lead), the
        // System surface (right-side Settings button), and Desktop (far-right
        // Show-Desktop sliver).
        use Surface::{
            About, Bookmarks, Browser, Chat, Editor, Files, InfraCode, Media, MeshView, Music,
            Phones, Storage, Terminal, Voice, Workbench,
        };
        let expect: [(&str, &[Surface]); 6] = [
            ("Comms", &[Voice, Chat, Phones]),
            ("Workloads", &[InfraCode]),
            ("Terminals", &[Browser, Bookmarks, Terminal, Editor]),
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
        // sliver + the six groups reproduce all 18 of Surface::ALL, each surface
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
        drive_vdock_with_sources(ctx, state, events, size, &[]);
    }

    fn drive_vdock_with_sources(
        ctx: &egui::Context,
        state: &mut DockState,
        events: Vec<egui::Event>,
        size: egui::Vec2,
        sources: &[DesktopRailSource],
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
            let _ = notification_rail_with_sources(ctx, state, sources);
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
                let _ = notification_rail(ctx, s);
            });
        };
        // Prime two frames so egui has the rail pin's rect registered, then move
        // onto the pin + press, then release the next frame.
        frame(&ctx, &mut s, Vec::new());
        frame(&ctx, &mut s, Vec::new());
        let click = ctx
            .read_response(egui::Id::new("vdock-pin"))
            .expect("the rail pin is registered")
            .rect
            .center();
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
        frame(&ctx, &mut s, vec![egui::Event::PointerMoved(click), press]);
        frame(&ctx, &mut s, vec![release]);
        assert!(s.pinned(), "clicking the pin holds the dock open (lock #9)");
    }

    // ── CONSOLE-1: the Start front door's dock cell ────────────────────────────

    #[test]
    fn the_start_cell_anchors_the_bottom_rail_and_latches_the_console_toggle() {
        // Console locks #1/#2 moved to the bottom rail: the Advanced/Start cell is
        // the far-left rail icon, before Desktop, and a click latches the console
        // toggle the shell drains — exactly once.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal the dock so its cells mount
        let sz = egui::vec2(1280.0, 900.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let start = ctx
            .read_response(start_cell_id())
            .expect("the Start cell is registered")
            .rect;
        assert!(
            start.left() < 8.0,
            "the Advanced cell anchors the rail's left"
        );
        assert!(
            start.height() < DOCK_W && start.width() < DOCK_W,
            "the Advanced cell is a small rail icon"
        );
        let desktop = ctx
            .read_response(pick_cell_id(Surface::Desktop))
            .expect("the Desktop rail cell is registered")
            .rect;
        assert!(
            desktop.left() >= start.right(),
            "Desktop sits immediately to the right of Advanced"
        );
        let wb = ctx
            .read_response(pick_cell_id(Surface::Workbench))
            .expect("the Workbench lead is registered")
            .rect;
        assert!(
            wb.top() < start.top(),
            "Workbench remains in the left rail while Advanced moved to the bottom rail"
        );

        assert!(!s.take_console_toggle(), "no toggle before a click");
        click_vdock(&ctx, &mut s, start.center(), sz);
        assert!(
            s.take_console_toggle(),
            "a Start-cell click latches the console toggle"
        );
        assert!(
            !s.take_console_toggle(),
            "the toggle latch drains exactly once"
        );
        assert_eq!(
            s.active,
            Surface::default(),
            "the Start cell routes NO surface — it only toggles the Console"
        );
    }

    // ── DOCK-OVERLAP: the shell reserves a gutter so the dock never overlaps ──

    #[test]
    fn a_shown_dock_reserves_a_full_gutter_a_hidden_one_reserves_nothing() {
        // DOCK-OVERLAP — the shell insets the central content by this width so the
        // dock never sits over the surface (except a full-screen remote desktop,
        // gated in main.rs). A fresh context reports the settled slide endpoint on
        // first sight (egui's `animate_bool`), so a shown dock reserves the full
        // DOCK_W and a hidden + settled dock reserves nothing (content fills width).
        let ctx = egui::Context::default();
        let mut shown = DockState::default();
        shown.toggle();
        assert!(
            (gutter_width(&ctx, &shown) - DOCK_W).abs() < f32::EPSILON,
            "a shown dock reserves a full DOCK_W gutter (no overlap)"
        );
        // A separate context so the slide latch starts fresh at the hidden endpoint.
        let ctx2 = egui::Context::default();
        let hidden = DockState::default();
        assert_eq!(
            gutter_width(&ctx2, &hidden),
            0.0,
            "a hidden + settled dock reserves nothing — the content fills full width"
        );
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

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Click `center` — press one frame, release the next (the egui click model
    /// the taskbar tests use). The caller primes the layout first.
    fn click_vdock(
        ctx: &egui::Context,
        state: &mut DockState,
        center: egui::Pos2,
        size: egui::Vec2,
    ) {
        click_vdock_with_sources(ctx, state, center, size, &[]);
    }

    fn click_vdock_with_sources(
        ctx: &egui::Context,
        state: &mut DockState,
        center: egui::Pos2,
        size: egui::Vec2,
        sources: &[DesktopRailSource],
    ) {
        drive_vdock_with_sources(
            ctx,
            state,
            vec![egui::Event::PointerMoved(center), press_at(center)],
            size,
            sources,
        );
        drive_vdock_with_sources(ctx, state, vec![release_at(center)], size, sources);
    }

    fn secondary_click_vdock(
        ctx: &egui::Context,
        state: &mut DockState,
        center: egui::Pos2,
        size: egui::Vec2,
    ) {
        let press = egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let release = egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        };
        drive_vdock(
            ctx,
            state,
            vec![egui::Event::PointerMoved(center), press],
            size,
        );
        drive_vdock(ctx, state, vec![release], size);
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
        // §7 — the Workbench lead + the thirteen group surfaces each route on a click
        // into DockState::active (the carried-over routing). Settings (System) +
        // Show-Desktop are NOT in the picker — they belong to VDOCK-4's system quad.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal the dock so its Area (and cells) mount
                    // Tall enough that all six groups render inline above the bottom zone
                    // (which VDOCK-5's clock strip grew by CLOCK_CELL_H).
        let sz = egui::vec2(1280.0, 900.0);
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

        // System stays out of the picker; Desktop is now the second bottom-rail
        // control in the Windows-style rail.
        assert!(
            ctx.read_response(pick_cell_id(Surface::System)).is_none(),
            "System (Settings) is deferred to VDOCK-4's system quad"
        );
        assert!(
            ctx.read_response(pick_cell_id(Surface::Desktop)).is_some(),
            "Desktop is a bottom-rail control"
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
        // Tall enough for all six groups inline over the clock-grown bottom zone.
        let sz = egui::vec2(1280.0, 900.0);
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
        // six render inline with no '…' overflow. The only other text in the
        // column is VDOCK-5's clock glyph (the live HH:MM, dim — lock #20).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 900.0);
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
        let accents: Vec<egui::Color32> = GROUPS.iter().map(|g| g.accent).collect();
        let (labels, rest): (Vec<_>, Vec<_>) = texts
            .into_iter()
            .partition(|(_, color)| accents.contains(color));
        assert_eq!(
            labels.len(),
            GROUPS.len(),
            "exactly one accent label per group (no captions, no '…' at this height)"
        );
        for (angle, _) in labels {
            assert!(
                angle.abs() < 1e-3,
                "the vertical dock's labels read HORIZONTALLY (angle 0), got {angle}"
            );
        }
        // No picker cell captions are painted; rail icons are glyphs, and the
        // clock lives outside the left-dock label cluster.
        assert_eq!(
            rest.len(),
            0,
            "besides group labels no left-dock captions are painted"
        );
        assert!(
            rest.iter().all(|(angle, _)| angle.abs() < 1e-3),
            "fixed chrome glyphs read upright"
        );
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
        assert!(
            bar.left() < 1.0,
            "the accent bar hugs the column's left edge"
        );
        assert!(
            (bar.height() - wb.height()).abs() < 1.0,
            "the bar spans the active cell's height"
        );
    }

    #[test]
    fn the_files_cell_badges_active_transfers_only_when_nonzero() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let sz = egui::vec2(1280.0, 900.0);

        let mut idle = DockState::default();
        idle.toggle();
        drive_vdock(&ctx, &mut idle, Vec::new(), sz);
        assert!(
            ctx.read_response(transfer_badge_id(Surface::Files))
                .is_none(),
            "zero active transfers paints no Files badge"
        );

        let mut active = DockState::default();
        active.toggle();
        active.set_transfer_active_count(15);
        drive_vdock(&ctx, &mut active, Vec::new(), sz);
        let files = ctx
            .read_response(pick_cell_id(Surface::Files))
            .expect("Files cell is registered")
            .rect;
        let badge = ctx
            .read_response(transfer_badge_id(Surface::Files))
            .expect("active transfers register a Files badge")
            .rect;
        assert!(
            files.contains(badge.center()),
            "the transfer badge is anchored inside the Files dock cell"
        );
        assert_eq!(super::badge_label(15), "15");
        assert_eq!(super::badge_label(120), "99+");
    }

    #[test]
    fn navbar5_live_badges_project_unread_peers_and_system_health() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let sz = egui::vec2(1280.0, 900.0);

        let mut idle = DockState::default();
        idle.toggle();
        drive_vdock(&ctx, &mut idle, Vec::new(), sz);
        assert!(
            ctx.read_response(surface_badge_id(Surface::Chat)).is_none(),
            "zero unread paints no Chat badge"
        );
        assert!(
            ctx.read_response(surface_badge_id(Surface::MeshView))
                .is_none(),
            "unseen mesh paints no Mesh badge"
        );
        assert!(
            ctx.read_response(surface_badge_id(Surface::System))
                .is_none(),
            "unseen health paints no System health dot"
        );

        let mut live = DockState::default();
        live.toggle();
        live.set_status_inputs(
            MeshSummary {
                peers_total: 5,
                peers_online: 4,
                health: mde_cosmic_applet::LighthouseHealth::Degraded,
                seen: true,
            },
            None,
            12,
            false,
            Vec::new(),
            NodeGrades::default(),
            StatusSegments::default(),
        );
        drive_vdock(&ctx, &mut live, Vec::new(), sz);
        drive_vdock(&ctx, &mut live, Vec::new(), sz);

        let chat = ctx
            .read_response(pick_cell_id(Surface::Chat))
            .expect("Chat picker cell is registered")
            .rect;
        let chat_badge = ctx
            .read_response(surface_badge_id(Surface::Chat))
            .expect("Chat unread count registers a badge")
            .rect;
        assert!(
            chat.contains(chat_badge.center()),
            "Chat unread badge is anchored inside the Chat glyph cell"
        );

        let mesh = ctx
            .read_response(pick_cell_id(Surface::MeshView))
            .expect("Mesh picker cell is registered")
            .rect;
        let mesh_badge = ctx
            .read_response(surface_badge_id(Surface::MeshView))
            .expect("Mesh peer count registers a badge")
            .rect;
        assert!(
            mesh.contains(mesh_badge.center()),
            "Mesh peer badge is anchored inside the Mesh glyph cell"
        );

        let system = ctx
            .read_response(sys_cell_id(SysCell::Settings))
            .expect("System settings cell is registered")
            .rect;
        let system_badge = ctx
            .read_response(surface_badge_id(Surface::System))
            .expect("mesh health registers a System health dot")
            .rect;
        assert!(
            system.contains(system_badge.center()),
            "System health dot is anchored inside the Settings glyph cell"
        );
        assert_eq!(super::badge_label(12), "12");
        assert_eq!(super::badge_label(4), "4");
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

    // ── VDOCK-5: the clock strip (Timers & Alarms home, locks #16/#20) ─────────

    #[test]
    fn the_clock_strip_shows_the_live_time_and_routes_to_timers() {
        // Lock #20 — the clock-glyph cell: it paints the LIVE wall-clock HH:MM
        // as its glyph (the time IS the icon), sits atop the bottom zone above
        // the status strip, and a click opens the Timers & Alarms surface.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 900.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let cell = ctx
            .read_response(clock_cell_id())
            .expect("the clock strip is registered")
            .rect;
        assert!(
            cell.width() < DOCK_W,
            "the rail clock is a compact tray item"
        );
        assert!(
            cell.left() > DOCK_W,
            "the clock is no longer in the left rail"
        );

        // A click routes to Timers & Alarms (the surface's ONE home).
        assert_ne!(s.active, Surface::Timers, "start off the Timers surface");
        click_vdock(&ctx, &mut s, cell.center(), sz);
        assert_eq!(
            s.active,
            Surface::Timers,
            "clicking the clock opens Timers & Alarms (lock #20)"
        );
    }

    #[test]
    fn timers_home_is_the_clock_cell_not_the_picker() {
        // Lock #20 — Timers deliberately sits OUTSIDE `Surface::ALL` (the picker
        // ordering authority) and every group: its one launcher is the clock
        // strip, so the picker/glyph tables stay exactly the 16 picker surfaces.
        assert!(
            !Surface::ALL.contains(&Surface::Timers),
            "Timers is not a picker surface — the clock strip is its home"
        );
        assert!(
            GROUPS
                .iter()
                .all(|g| !g.surfaces.contains(&Surface::Timers)),
            "no group lists Timers"
        );
    }

    // ── NOTIF-3: the status strip wired into the dock's bottom zone ────────────

    #[test]
    fn a_status_segment_pip_routes_through_the_dock_bottom_zone() {
        // NOTIF-3 wired end-to-end: the shell feeds the compact status strip via
        // `set_status_inputs` and a click on a bottom-zone segment pip routes
        // `DockState::active` (lock #15). Mount the real dock, read the Alerts pip
        // centre by its stable id, and click it -> `active` follows to Chat.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal the dock so its Area (and the status strip) mount
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            3,
            false,
            Vec::new(),
            NodeGrades::default(),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        // Prime so the segment pip rects register + settle under their stable ids.
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let alerts = ctx
            .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
            .expect("the Alerts status segment is registered in the dock's bottom zone")
            .rect
            .center();
        assert_ne!(s.active, Surface::Chat, "start off the Chat surface");
        click_vdock(&ctx, &mut s, alerts, sz);
        assert_eq!(
            s.active,
            Surface::Chat,
            "clicking the Alerts segment routes to the Chat surface (lock #15)"
        );
    }

    #[test]
    fn navbar4_status_tray_is_folded_into_the_bottom_rail() {
        // NAVBAR-4 — the separate chrome/status strip is retired: status pips,
        // the detail toggle, and the clock all live in the full-width bottom rail.
        // The old `status_bar` local-grade pip id must not register from the dock.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        s.set_status_inputs(
            MeshSummary {
                peers_total: 4,
                peers_online: 3,
                health: mde_cosmic_applet::LighthouseHealth::AllHealthy,
                seen: true,
            },
            None,
            2,
            true,
            Vec::new(),
            grades(vec![grade("me", 95, true, false)]),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let start = ctx
            .read_response(start_cell_id())
            .expect("Start/Advanced cell is in the bottom rail")
            .rect;
        let clock = ctx
            .read_response(clock_cell_id())
            .expect("clock is in the bottom rail")
            .rect;
        let detail = ctx
            .read_response(status_detail_toggle_id())
            .expect("status detail toggle is in the bottom rail")
            .rect;
        let alerts = ctx
            .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
            .expect("status pips are in the bottom rail")
            .rect;
        assert!(
            [start, detail, alerts, clock]
                .into_iter()
                .all(|r| (r.center().y - start.center().y).abs() < 2.0),
            "Advanced, status pips, detail toggle, and clock share one bottom rail"
        );
        assert!(
            detail.left() > start.right()
                && alerts.left() > detail.right()
                && clock.left() > alerts.right(),
            "status tray is right-aligned after the left-side launcher/session run"
        );
        assert!(
            ctx.read_response(status::local_grade_pip_id()).is_none(),
            "the retired separate status_bar local-grade pip is not mounted"
        );

        click_vdock(&ctx, &mut s, detail.center(), sz);
        assert!(
            s.status_panel_open,
            "bottom-rail detail toggle opens status panel"
        );
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        assert!(
            ctx.read_response(status::status_panel_id()).is_some(),
            "the folded status tray still exposes the old chrome detail content"
        );
    }

    #[test]
    fn navbar8_shell_density_selects_compact_or_expanded_bottom_rail() {
        // NAVBAR-8 — the rail rides the same density the shell installs from the
        // formfactor/control-surface path. Pointer density keeps the compact
        // icon rail; touch density grows the labelled 48px variant.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let sz = egui::vec2(1280.0, 800.0);

        let mut compact = DockState::default();
        compact.set_density(Density::Mouse);
        drive_vdock(&ctx, &mut compact, Vec::new(), sz);
        drive_vdock(&ctx, &mut compact, Vec::new(), sz);
        let compact_start = ctx
            .read_response(start_cell_id())
            .expect("compact Advanced cell registers")
            .rect;
        assert!(
            (compact_start.height() - NOTIFICATION_RAIL_H + 4.0).abs() < 1.0,
            "compact density keeps the short icon rail"
        );

        let ctx = egui::Context::default();
        Style::install_with_density(&ctx, Density::Touch);
        let mut expanded = DockState::default();
        expanded.set_density(Density::Touch);
        drive_vdock(&ctx, &mut expanded, Vec::new(), sz);
        drive_vdock(&ctx, &mut expanded, Vec::new(), sz);
        let expanded_start = ctx
            .read_response(start_cell_id())
            .expect("expanded Advanced cell registers")
            .rect;
        let expanded_desktop = ctx
            .read_response(pick_cell_id(Surface::Desktop))
            .expect("expanded Desktop cell registers through the same surface id")
            .rect;
        assert!(
            expanded_start.height() >= NOTIFICATION_RAIL_EXPANDED_H - 6.0,
            "expanded density selects the 48px rail variant"
        );
        assert!(
            (expanded_start.center().y - expanded_desktop.center().y).abs() < 2.0,
            "expanded Advanced and Desktop still share one bottom rail"
        );
        assert!(
            expanded_desktop.width() > compact_start.width(),
            "expanded cells have room for the label variant"
        );
    }

    #[test]
    fn a_requested_desktop_session_renders_as_a_named_bottom_rail_entry() {
        // NAVBAR-U3 / operator rail request — once the Desktop surface has a real
        // requested target, the rail shows a taskbar-style session entry instead of
        // only the generic Sessions glyph. Clicking it focuses Desktop.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            true,
            vec![entry.clone()],
            NodeGrades::default(),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let rect = ctx
            .read_response(session_entry_id(0, &entry))
            .expect("the named session entry is registered")
            .rect;
        assert!(
            rect.width() > NOTIFICATION_RAIL_H,
            "session entries render wider than the fallback micro icon"
        );
        assert_ne!(s.active(), Surface::Desktop, "starts off Desktop");
        click_vdock(&ctx, &mut s, rect.center(), sz);
        assert_eq!(
            s.active(),
            Surface::Desktop,
            "session entry focuses Desktop"
        );
        assert_eq!(
            s.take_desktop_session_focus().as_deref(),
            Some("session-1"),
            "session entry latches its broker session id for the shell"
        );
    }

    #[test]
    fn navbar7_bottom_rail_more_popup_keeps_overflow_sessions_reachable() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let entries = vec![
            SessionRailEntry::with_session_id("s1", "Alpha Desktop", "RDP"),
            SessionRailEntry::with_session_id("s2", "Bravo Desktop", "RDP"),
            SessionRailEntry::with_session_id("s3", "Charlie Desktop", "VNC"),
            SessionRailEntry::with_session_id("s4", "Delta Desktop", "RDP"),
        ];
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            true,
            entries.clone(),
            NodeGrades::default(),
            StatusSegments::default(),
        );
        let sz = egui::vec2(380.0, 720.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        assert!(
            ctx.read_response(rail_more_id()).is_some(),
            "narrow rails render a More cell instead of silently dropping sessions"
        );
        assert!(
            ctx.read_response(session_entry_id(3, &entries[3]))
                .is_none(),
            "the trailing session is folded out of the inline rail"
        );

        let more = ctx
            .read_response(rail_more_id())
            .expect("More cell is registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, more, sz);
        assert!(s.rail_more_open, "clicking More opens the overflow popup");
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let hidden = ctx
            .read_response(session_entry_id(3, &entries[3]))
            .expect("the hidden session is reachable in the More popup")
            .rect
            .center();
        assert!(hidden.x > 0.0, "hidden popup row has a concrete hit rect");
        ctx.memory_mut(|m| m.request_focus(session_entry_id(3, &entries[3])));
        drive_vdock(&ctx, &mut s, vec![key(egui::Key::Enter)], sz);
        assert_eq!(s.active(), Surface::Desktop);
        assert_eq!(
            s.take_desktop_session_focus().as_deref(),
            Some("s4"),
            "clicking a popup session uses the same focus latch as inline entries"
        );
        assert!(!s.rail_more_open, "routing from More closes the popup");
    }

    #[test]
    fn the_desktop_rail_cell_latches_a_reconnect_request() {
        // NAVBAR-U1 — a Desktop rail click is more than navigation: the shell
        // drains a distinct reconnect request and asks ChooserState for the newest
        // recent desktop. Programmatic Desktop navigation does not set this latch.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        assert!(
            !s.take_desktop_reconnect(),
            "no reconnect latch before a rail click"
        );
        let desktop = ctx
            .read_response(pick_cell_id(Surface::Desktop))
            .expect("the Desktop rail cell is registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, desktop, sz);
        assert_eq!(
            s.active(),
            Surface::Desktop,
            "Desktop still routes normally"
        );
        assert!(
            s.take_desktop_reconnect(),
            "Desktop rail click latches reconnect"
        );
        assert!(
            !s.take_desktop_reconnect(),
            "the reconnect latch drains once"
        );
    }

    #[test]
    fn the_desktop_rail_caret_opens_sources_and_latches_a_pick() {
        // NAVBAR-U2 — the Desktop rail is a split control: main icon reconnects,
        // caret opens the compact source flyout, and a row click returns only the
        // source id for the shell to hand back to ChooserState.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        let sources = vec![DesktopRailSource::new(
            "peer:oak",
            "oak",
            "lighthouse-oak",
            "RDP",
            true,
            true,
            false,
        )];
        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);

        let caret = ctx
            .read_response(desktop_source_toggle_id())
            .expect("the Desktop source caret is registered")
            .rect
            .center();
        click_vdock_with_sources(&ctx, &mut s, caret, sz, &sources);
        assert!(s.desktop_sources_open, "caret opens the source flyout");

        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
        let row = ctx
            .read_response(desktop_source_row_id(&sources[0]))
            .expect("the source row is registered")
            .rect
            .center();
        click_vdock_with_sources(&ctx, &mut s, row, sz, &sources);
        assert_eq!(
            s.take_desktop_source_pick().as_deref(),
            Some("peer:oak"),
            "row click latches the selected chooser source id"
        );
        assert_eq!(s.active(), Surface::Desktop, "source pick focuses Desktop");
        assert!(
            !s.desktop_sources_open,
            "routing from the source flyout closes it"
        );
    }

    #[test]
    fn the_desktop_split_button_is_keyboard_reachable() {
        // NAVBAR-U1 — keyboard focus + Enter/Space must activate both halves of the
        // Desktop split control and the compact source rows.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        let sources = vec![DesktopRailSource::new(
            "peer:oak",
            "oak",
            "lighthouse-oak",
            "RDP",
            true,
            false,
            true,
        )];
        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);

        ctx.memory_mut(|m| m.request_focus(pick_cell_id(Surface::Desktop)));
        drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Enter)], sz, &sources);
        assert_eq!(s.active(), Surface::Desktop);
        assert!(
            s.take_desktop_reconnect(),
            "Enter on the Desktop half latches reconnect"
        );

        ctx.memory_mut(|m| m.request_focus(desktop_source_toggle_id()));
        drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Space)], sz, &sources);
        assert!(
            s.desktop_sources_open,
            "Space on the caret opens the source flyout"
        );

        drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
        ctx.memory_mut(|m| m.request_focus(desktop_source_row_id(&sources[0])));
        drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Enter)], sz, &sources);
        assert_eq!(
            s.take_desktop_source_pick().as_deref(),
            Some("peer:oak"),
            "Enter on a source row latches that chooser source"
        );
    }

    #[test]
    fn navbar6_arrow_keys_traverse_picker_glyph_focus() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 900.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        ctx.memory_mut(|m| m.request_focus(pick_cell_id(Surface::Workbench)));
        drive_vdock(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], sz);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(pick_cell_id(Surface::Voice)),
            "ArrowDown advances from Workbench to the first grouped glyph"
        );

        drive_vdock(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], sz);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(pick_cell_id(Surface::Workbench)),
            "ArrowUp traverses back to Workbench"
        );
    }

    #[test]
    fn navbar6_right_click_glyph_menu_offers_pin_info_and_close_when_closable() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 900.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let workbench = ctx
            .read_response(pick_cell_id(Surface::Workbench))
            .expect("Workbench cell is registered")
            .rect
            .center();
        secondary_click_vdock(&ctx, &mut s, workbench, sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        assert!(
            ctx.read_response(surface_context_item_id(
                Surface::Workbench,
                SurfaceContextItem::Pin
            ))
            .is_some(),
            "right-clicking a glyph opens a Pin row"
        );
        assert!(
            ctx.read_response(surface_context_item_id(
                Surface::Workbench,
                SurfaceContextItem::Info
            ))
            .is_some(),
            "right-clicking a glyph opens an Info row"
        );
        assert!(
            ctx.read_response(surface_context_item_id(
                Surface::Workbench,
                SurfaceContextItem::Close
            ))
            .is_none(),
            "anchor glyphs omit Close rather than showing a placebo"
        );

        let pin = ctx
            .read_response(surface_context_item_id(
                Surface::Workbench,
                SurfaceContextItem::Pin,
            ))
            .expect("Pin row remains registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, pin, sz);
        assert!(s.pinned(), "Pin toggles the dock hold-open state");
        click_vdock(&ctx, &mut s, egui::pos2(600.0, 400.0), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let initial_browser = ctx
            .read_response(pick_cell_id(Surface::Browser))
            .expect("Browser cell is registered")
            .rect
            .center();
        assert!(initial_browser.x > 0.0, "Browser has a concrete cell");
        s.set_active(Surface::Browser);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        assert_eq!(s.active(), Surface::Browser);
        let browser = ctx
            .read_response(pick_cell_id(Surface::Browser))
            .expect("Browser cell remains registered after activation")
            .rect
            .center();
        secondary_click_vdock(&ctx, &mut s, browser, sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let close = ctx
            .read_response(surface_context_item_id(
                Surface::Browser,
                SurfaceContextItem::Close,
            ))
            .expect("closable Browser glyph exposes Close")
            .rect
            .center();
        click_vdock(&ctx, &mut s, close, sz);
        assert_eq!(
            s.active(),
            Surface::Workbench,
            "Close on the closable Desktop glyph returns to Workbench"
        );
    }

    #[test]
    fn the_status_chevron_opens_and_dismisses_the_detail_panel() {
        // NOTIF-4 — the detail panel is now mounted from the bottom rail; Esc and
        // click-away both dismiss it.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            false,
            Vec::new(),
            grades(vec![grade("me", 95, true, false)]),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        assert!(!s.status_panel_open, "panel starts closed");
        let caret = ctx
            .read_response(status_detail_toggle_id())
            .expect("bottom-rail status caret renders")
            .rect
            .center();
        click_vdock(&ctx, &mut s, caret, sz);
        assert!(
            s.status_panel_open,
            "bottom-rail caret opens the status panel"
        );
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        assert!(
            ctx.read_response(status::status_panel_id()).is_some(),
            "the status detail panel renders after opening"
        );

        drive_vdock(&ctx, &mut s, vec![key(egui::Key::Escape)], sz);
        assert!(!s.status_panel_open, "Escape dismisses the panel");

        s.open_status_panel_for_test();
        assert!(s.status_panel_open, "test seam reopens the panel");
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        click_vdock(&ctx, &mut s, egui::pos2(500.0, 500.0), sz);
        assert!(!s.status_panel_open, "click-away dismisses the panel");
    }

    #[test]
    fn the_status_panel_routes_device_controls_and_grade_rows() {
        // NOTIF-4 — device controls route to System; grade rows request the same
        // Explorer node-focus path as the dock mini-list.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            false,
            Vec::new(),
            grades(vec![
                grade("me", 95, true, false),
                grade("oak", 42, false, false),
            ]),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        s.open_status_panel_for_test();
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let device = ctx
            .read_response(status::status_panel_device_id())
            .expect("device control band registered")
            .rect
            .center();
        assert_ne!(s.active, Surface::System, "start off System");
        click_vdock(&ctx, &mut s, device, sz);
        assert_eq!(s.active, Surface::System, "device band routes to System");
        assert!(
            !s.status_panel_open,
            "routing from the panel closes the auxiliary panel"
        );

        s.open_status_panel_for_test();
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let oak = ctx
            .read_response(status::status_panel_grade_id("oak"))
            .expect("peer grade row registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, oak, sz);
        assert_eq!(
            s.take_node_focus().as_deref(),
            Some("oak"),
            "tapping a panel grade row records a node-focus request"
        );
    }

    // ── VDOCK-4: the system 2×2 quad + Power menu (design #7/#17/#18) ──────────

    #[test]
    fn the_system_quad_cells_are_settings_desktop_lock_power() {
        // Design #7/#17 — the four cells, row-major, sized to match the ~18px status
        // quad icons (#12/#23, smaller than the 24px app glyph).
        assert_eq!(
            SYSTEM_QUAD,
            [
                SysCell::Settings,
                SysCell::ShowDesktop,
                SysCell::Lock,
                SysCell::Power
            ],
            "the system quad is Settings · Show-Desktop · Lock · Power"
        );
        assert!(
            (SYS_QUAD_ICON - 18.0).abs() < f32::EPSILON,
            "the quad glyph edge is ~18px (design #23)"
        );
        assert!(
            SYS_QUAD_ICON < ICON_LOGICAL,
            "the quad icon is smaller than the 24px app glyph (#12)"
        );
    }

    #[test]
    fn system_quad_cells_use_status_pip_tint_language() {
        // NOTIF-12 — inactive controls share the status-pip dim baseline; hover or
        // active state reveals each action's semantic Carbon tone.
        assert_eq!(
            sys_cell_tint(SysCell::Settings, false, false),
            Style::TEXT_DIM
        );
        assert_eq!(
            sys_cell_tint(SysCell::Settings, true, false),
            Style::SUPPORT_INFO
        );
        assert_eq!(
            sys_cell_tint(SysCell::ShowDesktop, true, false),
            Style::SUPPORT_SUCCESS
        );
        assert_eq!(
            sys_cell_tint(SysCell::Lock, false, true),
            Style::SUPPORT_WARNING
        );
        assert_eq!(
            sys_cell_tint(SysCell::Power, true, false),
            Style::SUPPORT_ERROR
        );
    }

    #[test]
    fn the_system_quad_lays_out_as_a_2x2_in_the_final_dock_row() {
        // Design #7/#8 — the four cells form a 2×2 of DOCK_W/2 cells in the reserved
        // final DOCK_W row (directly beneath NOTIF-3's status strip).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle(); // reveal the dock so its Area (and the quad) mount
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let rect_of = |cell| {
            ctx.read_response(sys_cell_id(cell))
                .expect("system-quad cell registered")
                .rect
        };
        let (tl, tr, bl, br) = (
            rect_of(SysCell::Settings),
            rect_of(SysCell::ShowDesktop),
            rect_of(SysCell::Lock),
            rect_of(SysCell::Power),
        );
        let cell = DOCK_W / 2.0;
        for r in [tl, tr, bl, br] {
            assert!((r.width() - cell).abs() < 1.0, "cell is DOCK_W/2 wide");
            assert!((r.height() - cell).abs() < 1.0, "cell is DOCK_W/2 tall");
        }
        // Two columns: left cells share a left edge, right cells one cell over.
        assert!((tl.left() - bl.left()).abs() < 1.0, "left column aligned");
        assert!(
            (tr.left() - tl.right()).abs() < 1.0,
            "right column one cell over"
        );
        assert!((br.left() - tr.left()).abs() < 1.0, "right column aligned");
        // Two rows: top cells share a top edge, bottom cells one row down.
        assert!((tl.top() - tr.top()).abs() < 1.0, "top row aligned");
        assert!(
            (bl.top() - tl.bottom()).abs() < 1.0,
            "bottom row one cell down"
        );
        assert!((br.top() - bl.top()).abs() < 1.0, "bottom row aligned");
        // The quad sits directly above the Windows-style bottom rail.
        assert!(
            (tl.top() - (sz.y - DOCK_W - 20.0)).abs() < 1.0,
            "the system quad occupies the row above the bottom rail"
        );
        // It spans the full column width (two DOCK_W/2 columns).
        assert!(
            (tr.right() - tl.left() - DOCK_W).abs() < 1.0,
            "the quad spans the column width"
        );
    }

    #[test]
    fn each_system_quad_cell_dispatches_its_route_or_action() {
        // §7 — every system-quad cell drives its real target on a click: Settings →
        // System, Show-Desktop → the existing Desktop route, Lock → a curtain lock
        // request the shell drains, Power → the armed menu opens.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        // Read all four centres up front (a click shifts no rect); click Power last
        // so its popup can't overlap the earlier cells.
        let centre = |cell| {
            ctx.read_response(sys_cell_id(cell))
                .expect("system-quad cell registered")
                .rect
                .center()
        };
        let (settings, desktop, lock, power) = (
            centre(SysCell::Settings),
            centre(SysCell::ShowDesktop),
            centre(SysCell::Lock),
            centre(SysCell::Power),
        );

        // Settings → System.
        assert_ne!(s.active, Surface::System, "start off System");
        click_vdock(&ctx, &mut s, settings, sz);
        assert_eq!(s.active, Surface::System, "Settings routes to System");

        // Show-Desktop → Desktop (the existing route).
        click_vdock(&ctx, &mut s, desktop, sz);
        assert_eq!(s.active, Surface::Desktop, "Show-Desktop routes to Desktop");

        // Lock → a pending curtain lock request the shell drains (once).
        click_vdock(&ctx, &mut s, lock, sz);
        assert_eq!(
            s.take_request(),
            Some(DockRequest::Lock),
            "Lock records a curtain lock request"
        );
        assert!(
            s.take_request().is_none(),
            "the request drains once (the shell reads it a single time)"
        );

        // Power → the armed menu opens.
        assert!(!s.power.open, "the Power menu is closed by default");
        click_vdock(&ctx, &mut s, power, sz);
        assert!(s.power.open, "clicking Power opens its menu (#18)");
    }

    #[test]
    fn the_power_menu_arms_reboot_and_shutdown_before_firing() {
        // Design #18 — the two host-down verbs demand a typed echo before they fire.
        // The pure arming gate: an empty / mistyped echo never arms; only the exact
        // (case-insensitive) verb name does.
        let mut menu = PowerMenu::default();
        menu.arm(PowerItem::Reboot);
        assert!(!menu.armed(), "an empty echo never arms Reboot");
        menu.arming.as_mut().expect("arming set").echo = "nope".to_owned();
        assert!(!menu.armed(), "a mistyped echo never arms Reboot");
        menu.arming.as_mut().expect("arming set").echo = "reboot".to_owned();
        assert!(menu.armed(), "the exact verb name (any case) arms it");

        // The fired verb drives the REAL seam the shell drains: Reboot → PowerVerb::
        // Reboot, Shutdown → PowerVerb::PowerOff; each drains once.
        let mut s = DockState::default();
        s.power.arm(PowerItem::Reboot);
        s.power.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
        assert!(s.power.armed(), "the dock's arming gate matches");
        s.fire_power(PowerItem::Reboot);
        assert_eq!(
            s.take_request(),
            Some(DockRequest::Power(PowerVerb::Reboot)),
            "a confirmed Reboot records the real logind verb"
        );
        assert!(s.take_request().is_none(), "the request drains once");
        assert!(!s.power.open, "firing a verb closes the menu");

        s.fire_power(PowerItem::Shutdown);
        assert_eq!(
            s.take_request(),
            Some(DockRequest::Power(PowerVerb::PowerOff)),
            "Shutdown maps to logind PowerOff"
        );

        // Suspend acts at once (no arming); Lock routes to the curtain, not a verb.
        s.fire_power(PowerItem::Suspend);
        assert_eq!(
            s.take_request(),
            Some(DockRequest::Power(PowerVerb::Suspend))
        );
        s.fire_power(PowerItem::Lock);
        assert_eq!(
            s.take_request(),
            Some(DockRequest::Lock),
            "the menu's Lock item drops the curtain, not a logind verb"
        );
    }

    #[test]
    fn clicking_reboot_in_the_menu_only_arms_it_and_fires_nothing() {
        // Design #18 end-to-end: opening the Power menu and clicking Reboot enters
        // the typed-arming stage — it does NOT reboot (no power request fires until
        // the echo is confirmed). Guards the "one click reboots" trap.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        // Open the menu off the Power cell.
        let power = ctx
            .read_response(sys_cell_id(SysCell::Power))
            .expect("Power cell registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, power, sz);
        assert!(s.power.open, "the menu opened");

        // Settle so the popup rows register, then click Reboot.
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let reboot = ctx
            .read_response(power_item_id(PowerItem::Reboot))
            .expect("the Reboot menu row renders in the popup")
            .rect
            .center();
        click_vdock(&ctx, &mut s, reboot, sz);

        assert!(
            s.power.arming.as_ref().map(|a| a.verb) == Some(PowerItem::Reboot),
            "clicking Reboot enters its typed-arming stage"
        );
        assert!(
            s.take_request().is_none(),
            "Reboot fires NOTHING until the echo is typed-armed (#18)"
        );

        // The top-level Power menu offers exactly the four locked items.
        assert_eq!(
            POWER_MENU,
            [
                PowerItem::Lock,
                PowerItem::Suspend,
                PowerItem::Reboot,
                PowerItem::Shutdown
            ],
            "the Power menu is Lock / Suspend / Reboot / Shutdown (#18)"
        );
    }

    // ── NODE-GRADE-2: the grade mini-list band (design #5/#7/#8/#18/#19) ───────

    #[test]
    fn the_grade_band_has_no_height_without_grades() {
        // Pre-poll / empty grades → the band claims 0, so the dock's layout is
        // byte-identical to the pre-NODE-GRADE dock (§7 honest: no fake rows).
        assert!(
            grade_band_height(&NodeGrades::default()).abs() < f32::EPSILON,
            "an empty grade set paints no band"
        );
        assert!(
            grade_band_height(&grades(vec![grade("me", 90, true, false)])) > 0.0,
            "one grade claims a band"
        );
    }

    #[test]
    fn the_grade_rows_sit_above_the_bottom_status_tray_local_first() {
        // Design #18/#19 — the grade mini-list paints in the bottom zone ABOVE the
        // NOTIF-3 status strip, in the given render order (local pinned first). The
        // rows register addressable rects and every one clears the strip.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        // Local "me" pinned first, then a worst-first peer.
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            false,
            Vec::new(),
            grades(vec![
                grade("me", 95, true, false),
                grade("oak", 40, false, false),
            ]),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let me = ctx
            .read_response(grade_row_id("me"))
            .expect("the local grade row is registered")
            .rect;
        let oak = ctx
            .read_response(grade_row_id("oak"))
            .expect("the peer grade row is registered")
            .rect;
        // Local pinned first (renders above the peer), matching the fold order.
        assert!(
            me.top() < oak.top(),
            "the local node's row is pinned first (#18)"
        );
        // Both rows sit above the bottom rail's status tray.
        let tray = ctx
            .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
            .expect("the bottom status tray is registered")
            .rect;
        assert!(
            tray.width() < DOCK_W,
            "the bottom status tray uses micro icons"
        );
        // Each row spans the full column width (the dock idiom).
        assert!(
            (me.width() - DOCK_W).abs() < 1.0,
            "a grade row is the full column"
        );
    }

    #[test]
    fn tapping_a_grade_row_records_a_node_focus_request() {
        // Design #7 — a grade row tap records the host's Explorer-hero focus request
        // the shell drains (routing to the Mesh Map's Explorer lens). The request
        // drains exactly once (the shell reads it a single frame).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            false,
            Vec::new(),
            grades(vec![grade("oak", 40, false, false)]),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        let oak = ctx
            .read_response(grade_row_id("oak"))
            .expect("the grade row is registered")
            .rect
            .center();
        assert!(
            s.take_node_focus().is_none(),
            "no focus request before the tap"
        );
        click_vdock(&ctx, &mut s, oak, sz);
        assert_eq!(
            s.take_node_focus().as_deref(),
            Some("oak"),
            "tapping a grade row records that node's hero-focus request (#7)"
        );
        assert!(
            s.take_node_focus().is_none(),
            "the focus request drains once"
        );
    }

    #[test]
    fn the_grade_overflow_expander_reveals_the_hidden_peers() {
        // Design #8 — past the worst-N cap the extra peers fold into a '…' expander:
        // the '…' cell is present, the capped peer is hidden until it opens, and a
        // popup row still routes to its hero (then closes the popup).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = DockState::default();
        s.toggle();
        // One local + (GRADE_MAX_ROWS) peers → one peer spills past the cap.
        let mut rows = vec![grade("me", 99, true, false)];
        let peers = ["p1", "p2", "p3", "p4", "p5"];
        assert_eq!(peers.len(), GRADE_MAX_ROWS, "seed exactly one over the cap");
        for (i, name) in peers.iter().enumerate() {
            // Ascending scores so the render order is stable; the last is hidden.
            rows.push(grade(
                name,
                10 + u8::try_from(i).unwrap_or(0) * 10,
                false,
                false,
            ));
        }
        let hidden_host = peers[peers.len() - 1];
        s.set_status_inputs(
            MeshSummary::default(),
            None,
            0,
            false,
            Vec::new(),
            grades(rows),
            StatusSegments::default(),
        );
        let sz = egui::vec2(1280.0, 800.0);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);

        assert!(
            ctx.read_response(grade_overflow_id()).is_some(),
            "the '…' expander is present when peers spill past the cap"
        );
        assert!(
            ctx.read_response(grade_row_id(hidden_host)).is_none(),
            "the capped peer is hidden until the expander opens"
        );

        // Open the expander.
        let more = ctx
            .read_response(grade_overflow_id())
            .expect("the '…' cell is registered")
            .rect
            .center();
        click_vdock(&ctx, &mut s, more, sz);
        assert!(s.grades_overflow_open, "clicking '…' opens the expander");

        // Settle the popup, then tap the hidden peer inside it.
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        drive_vdock(&ctx, &mut s, Vec::new(), sz);
        let hidden = ctx
            .read_response(grade_row_id(hidden_host))
            .expect("the hidden peer renders in the expander popup")
            .rect
            .center();
        click_vdock(&ctx, &mut s, hidden, sz);
        assert_eq!(
            s.take_node_focus().as_deref(),
            Some(hidden_host),
            "a tap in the expander routes to that node's hero"
        );
        assert!(
            !s.grades_overflow_open,
            "routing from the expander closes it"
        );
    }
}
