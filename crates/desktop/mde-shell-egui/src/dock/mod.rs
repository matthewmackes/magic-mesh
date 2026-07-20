//! The shell taskbar/dock state module. The rendered left vertical dock was
//! retired by WIN10-HYBRID; the live chrome exported from this module is the
//! full-width **bottom taskbar** ([`notification_rail_with_sources`], design
//! `docs/design/win10-taskbar.md` plus the taskbar locks from
//! `docs/design/win7-desktop-survey.md`).
//!
//! Under E12 "Construct" the mesh-control surfaces are **panels in the one shell**,
//! not separate clients (§5, the EMBED model — there is no compositor). The
//! bottom taskbar is that shell chrome: the session rail focuses desktops, the
//! tray/status area exposes live system
//! state, and taskbar cells update the active [`Surface`]. One surface shows at
//! a time; the Start Menu/front door/hotkeys are the surface launchers.
//!
//! The retired picker grouping and left-rail constants remain only where tests or
//! shared icon geometry still need them. Production navigation goes through the
//! bottom taskbar, front door, and hotkeys.
//!
//! The dock is pure chrome: it reads + writes the active [`Surface`] and draws
//! through the shared [`Style`] (§4). It never builds or drives a surface — the
//! shell owns each surface's app and its per-frame pump.
//!
//! The bottom taskbar is docked by default and can auto-hide from the persisted
//! Settings preference; the retired left dock no longer exposes a reveal or pin
//! control. A clean Super tap opens the Front Door launcher.

use mde_egui::egui::{self, TextureHandle, TextureOptions};
use mde_egui::{Density, Motion, Style};
use mde_seat::SeatSnapshot;
use mde_theme::brand::icons::{icon_image, IconId};

use crate::chrome::{MeshSummary, NodeGrades};
use crate::status::{self, StatusSegment, StatusSegments};

pub type FileOperationProgress = status::FileOperationStatus;

/// Which surface fills the shell body.
///
/// [`Desktop`](Self::Desktop) is the default: the shell opens on the neutral
/// Remote Sessions view, while mesh-control work starts only after an explicit
/// route to the Workbench.
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form for
/// crate-visible items in a private module — like `TASKBAR_H` below.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Surface {
    /// The five-plane mesh-control Workbench (This Node → Fleet).
    Workbench,
    /// The live **Mesh Map** — the egui reincarnation of MESHMAP (`mde-mesh-view`):
    /// a procedural canvas of the current mesh (nodes by role + health, the elected
    /// leader, and the links between them), folded from the same world-readable
    /// mesh-status snapshot the Workbench planes read. An all-green onboard
    /// self-test auto-opens it (OW-10).
    MeshView,
    /// The **Explorer** — the EXPLORER-epic discovery surface (`crate::explorer`):
    /// a cinematic one-unit-at-a-time hero view over every discovered unit (mesh
    /// peers · off-mesh LAN hosts · cloud objects), folded from the
    /// aggregator's `state/units/*` mirrors. A first-class dock surface (its Mesh
    /// sibling beside the Mesh Map); it is ALSO reachable as the Mesh Map's
    /// segmented Explorer lens, which powers the NODE-GRADE-2 node-focus jump.
    Explorer,
    /// The VDI **Desktop** surface — a brokered VM desktop rendered egui-native
    /// (`mde-vdi-rdp` / `mde-vdi-vnc`), the point of E12 "Construct".
    #[default]
    Desktop,
    /// The **Infra as Code (`IaC`)** surface — the cloud `IaaS` control plane
    /// (`docs/design/iac-workspace.md`, IAC-2): the service catalog + per-service
    /// API health + the merged service directory, consumed off the Bus
    /// (`action/cloud/get-catalog`). The comprehensive infrastructure admin beside
    /// the member-facing Cloud plane (#24).
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
    /// The embedded Maps & Location surface (`mde-maps-location-egui`) — native
    /// offline navigation, location-source control, MG90 direct-Ethernet
    /// management, vehicle telemetry, recovery, and simulator workflows for one
    /// vehicle.
    MapsLocation,
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
    /// cloud lifecycle set), fast mesh-wide unpair, and the pair-a-phone flow. A
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
    /// placement lock #13): the official `CONSTRUCT-MAIN.png` lockup, the product
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
    /// and the System / Storage / About screens. [`LAUNCHER_GROUPS`] gathers these
    /// into the shared launcher taxonomy, preserving this relative order within
    /// each group; a compile-time guard keeps the two tables in sync.
    pub(crate) const ALL: [Surface; 19] = [
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
        Surface::MapsLocation,
        Surface::Terminal,
        Surface::Editor,
        Surface::Chat,
        Surface::Phones,
        Surface::System,
        Surface::Storage,
        Surface::About,
    ];

    /// The [`brand::icons`](mde_theme::brand::icons) glyph this surface draws in
    /// the bar (QBRAND-7). A 1:1 map by name onto the Construct/YAMIS icon set — every
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
            // the cloud IaaS control plane reads as "infrastructure".
            Surface::InfraCode => IconId::Server,
            Surface::Desktop => IconId::Desktop,
            Surface::Music => IconId::Music,
            Surface::Media => IconId::Media,
            Surface::Files => IconId::Files,
            Surface::Voice => IconId::Voice,
            Surface::Browser => IconId::Browser,
            Surface::Bookmarks => IconId::Bookmarks,
            Surface::MapsLocation => IconId::MapsLocation,
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
            Surface::Desktop => "Remote Sessions",
            Surface::Music => "Music",
            Surface::Media => "Media",
            Surface::Files => "Files",
            Surface::Voice => "Voice",
            Surface::Browser => "Browser",
            Surface::Bookmarks => "Bookmarks",
            Surface::MapsLocation => "Maps & Location",
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

/// One shared launcher category used by Start, Front Door, and tests. Keeping
/// the taxonomy next to [`Surface`] prevents a surface from being "Files" in
/// one launcher and "System" in another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LauncherGroup {
    pub(crate) label: &'static str,
    pub(crate) accent: egui::Color32,
    pub(crate) surfaces: &'static [Surface],
}

/// The one truthful surface grouping for shell launchers. Every
/// [`Surface::ALL`] entry appears exactly once; [`Surface::Timers`] remains
/// clock-owned and outside the launcher grid.
pub(crate) const LAUNCHER_GROUPS: [LauncherGroup; 8] = [
    LauncherGroup {
        label: "Mesh Control",
        accent: Style::ACCENT_MESH,
        surfaces: &[Surface::Workbench, Surface::MeshView, Surface::InfraCode],
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
        surfaces: &[Surface::Files, Surface::Storage],
    },
    LauncherGroup {
        label: "Web",
        accent: Style::ACCENT_WEB,
        surfaces: &[Surface::Browser, Surface::Bookmarks],
    },
    LauncherGroup {
        label: "Developer Tools",
        accent: Style::ACCENT_TERMINALS,
        surfaces: &[Surface::Terminal, Surface::Editor],
    },
    LauncherGroup {
        label: "Comms",
        accent: Style::ACCENT_COMMS,
        surfaces: &[Surface::Voice, Surface::Chat, Surface::Phones],
    },
    LauncherGroup {
        label: "System",
        accent: Style::ACCENT_WORKLOADS,
        surfaces: &[Surface::System, Surface::About, Surface::Explorer],
    },
];

const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        let mut count = 0;
        let mut g = 0;
        while g < LAUNCHER_GROUPS.len() {
            let surfaces = LAUNCHER_GROUPS[g].surfaces;
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
            "every Surface::ALL entry must appear in LAUNCHER_GROUPS exactly once",
        );
        i += 1;
    }
};

pub(crate) fn launcher_group_label(surface: Surface) -> &'static str {
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map_or("", |group| group.label)
}

#[cfg(test)]
pub(crate) fn launcher_group_accent(surface: Surface) -> Option<egui::Color32> {
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map(|group| group.accent)
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
/// legacy [`DOCK_W`] column is this same module. Private: only the taskbar/dock
/// layout + tests read it.
const CELL_W: f32 = Style::SP_XL + Style::SP_M;

/// The app glyph edge in logical points — the 24px dock icon (lock W3, `SP_L`).
/// Rasterized crisp at the physical pixel size by `icon_texture`.
const ICON_LOGICAL: f32 = Style::SP_L;

/// The live bottom taskbar is intentionally a black shell strip with white control
/// glyphs. Status pips may still use semantic health colors, but taskbar-owned
/// controls do not inherit the old dim/accent icon tint ramp.
const TASKBAR_BG: egui::Color32 = Style::TASKBAR_BG;
const TASKBAR_BORDER: egui::Color32 = Style::TASKBAR_BORDER;
const TASKBAR_HOVER_FILL: egui::Color32 = Style::TASKBAR_HOVER_FILL;
const TASKBAR_ACTIVE_FILL: egui::Color32 = Style::TASKBAR_ACTIVE_FILL;
const TASKBAR_ICON: egui::Color32 = Style::TASKBAR_ICON;
pub(crate) const TASKBAR_TRAY_ISLAND_FILL: egui::Color32 = Style::TASKBAR_TRAY_ISLAND_FILL;
pub(crate) const TASKBAR_TRAY_ISLAND_ACTIVE_FILL: egui::Color32 =
    Style::TASKBAR_TRAY_ISLAND_ACTIVE_FILL;
const TASKBAR_TRAY_ISLAND_BORDER: egui::Color32 = Style::TASKBAR_TRAY_ISLAND_BORDER;
const TASKBAR_CLOCK_DATE: egui::Color32 = Style::TASKBAR_CLOCK_DATE;
const DESKTOP_SOURCE_TOGGLE_ICON: IconId = IconId::Desktop;
const STATUS_DETAIL_ICON: IconId = IconId::HealthStatus;
const TRAY_OVERFLOW_ICON: IconId = IconId::MoreHorizontal;
const ACTION_CENTER_ICON: IconId = IconId::Notifications;

#[must_use]
const fn taskbar_control_icon_tint(
    _selected: bool,
    _hovered: bool,
    _disabled: bool,
) -> egui::Color32 {
    TASKBAR_ICON
}

#[must_use]
const fn taskbar_cell_fill(selected: bool, hovered: bool) -> Option<egui::Color32> {
    if selected {
        Some(TASKBAR_ACTIVE_FILL)
    } else if hovered {
        Some(TASKBAR_HOVER_FILL)
    } else {
        None
    }
}

fn win11_tray_island_width(rail_h: f32, clock_w: f32, status_w: f32) -> f32 {
    rail_h * 3.0 + clock_w + status_w + Style::SP_XS
}

fn win11_tray_island_rect(
    rail: egui::Rect,
    rail_h: f32,
    clock_w: f32,
    status_w: f32,
) -> egui::Rect {
    let right = rail.right() - Style::SP_S;
    let width = win11_tray_island_width(rail_h, clock_w, status_w);
    egui::Rect::from_min_max(
        egui::pos2(right - width, rail.top() + 2.0),
        egui::pos2(right, rail.bottom() - 2.0),
    )
}

fn paint_win11_tray_island(ui: &egui::Ui, rect: egui::Rect, active: bool) {
    let fill = if active {
        TASKBAR_TRAY_ISLAND_ACTIVE_FILL
    } else {
        TASKBAR_TRAY_ISLAND_FILL
    };
    ui.painter().rect_filled(rect, Style::RADIUS, fill);
    ui.painter().rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(HAIRLINE_W, TASKBAR_TRAY_ISLAND_BORDER),
        egui::StrokeKind::Inside,
    );
}

/// The Carbon-blue group hairline width in logical points — a 1px rule (L3).
const HAIRLINE_W: f32 = 1.0;

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
    let tint = Style::resolve_color(ctx, tint).to_array();
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
// Retired VDOCK geometry and compatibility state.
//
// The left-edge chrome renderer no longer mounts in production, but a small
// compatibility reveal marker and geometry constants survive because the bottom
// taskbar reuses the active-surface model, icon sizing, auto-hide semantics, and
// regression tests that prove no left gutter is reserved.
// ═══════════════════════════════════════════════════════════════════════════

/// Legacy left-dock width in logical points (~48px). The rendered dock is
/// retired; tests still use this constant to prove a stale gutter would shift
/// content by exactly one legacy column. (`pub`, not `pub(crate)` — the
/// `clippy::redundant_pub_crate` form for a crate-visible item in a private
/// module, like [`TASKBAR_H`].)
pub const DOCK_W: f32 = CELL_W;

/// The egui memory key for NOTIF-4's right-side status detail panel.
const STATUS_PANEL_KEY: &str = "vdock-status-panel";

/// The stable id of the bottom notification rail layer.
const NOTIFICATION_RAIL_AREA: &str = "notif-bottom-rail-area";

/// The shell taskbar state. The retired left-dock pin is gone; a legacy reveal
/// marker remains only so old hotkey/test seams can prove they still reserve no
/// gutter. Live production rendering uses the bottom taskbar. The active surface,
/// session rail, progress/status inputs, and overflow popups all flow through this
/// struct. Start Menu remains the owner of app pins; this state carries only the
/// frame-local projection needed to paint those pins on the application bar.
// The dock carries several INDEPENDENT boolean latches (legacy reveal, taskbar
// auto-hide, and overflow/detail popups) —
// not a state machine folding into one enum, so opt this one struct past the
// `struct_excessive_bools` bar rather than contrive a two-variant enum.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct DockState {
    /// Toggled by a clean Super tap (lock #13) — the hotkey reveal/hide latch.
    revealed: bool,
    /// The **active surface** the taskbar selects — a taskbar cell click writes it
    /// here; the shell body follows [`Self::active`]. Defaults to
    /// [`Surface::Desktop`] (the shell opens on Remote Sessions).
    active: Surface,
    /// Application-bar pins mirrored from the Start Menu's ordered pin store.
    /// The dock never persists or mutates this list; it only paints/routes it.
    pinned_surfaces: Vec<Surface>,
    /// WIN10-HYBRID #31 — whether the ▲ **tray-overflow** flyout is open (the Win10
    /// hidden-icons popup): set by the ▲ cell, cleared on a route or a click-away.
    tray_overflow_open: bool,
    /// NOTIF-4 — whether the bottom notification rail's detail panel is open.
    /// Toggled by the rail Health control and dismissed by Esc or click-away.
    status_panel_open: bool,
    /// The live inputs NOTIF-3's bottom **notification rail** folds each frame —
    /// owned so the taskbar keeps its `(ctx, state)` signature; the shell refreshes
    /// it via [`Self::set_status_inputs`] each frame. Defaults to the honest pre-poll
    /// state.
    status: StatusInputs,
    /// Optional live thumbnail for the currently attached Desktop session. Kept
    /// separate from [`SessionRailEntry`] so rail entries remain cheap,
    /// comparable summaries.
    session_preview: Option<SessionPreviewTexture>,
    /// A pending **node-focus** request the notification panel's grade list records
    /// when a grade row is tapped (design #7): the hostname whose Explorer hero the
    /// shell should open. The dock can't reach the shell's Explorer / nav (§6), so it
    /// records the host here and `main.rs` drives the jump via
    /// [`Self::take_node_focus`] (the deferred-wire idiom).
    /// A `String` (not `Copy`), so it rides its own field.
    pending_node_focus: Option<String>,
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
    /// FILE-STATUS-1 — the Files surface's active local/transfer operation summary,
    /// mirrored each frame so the bottom taskbar can show one reusable progress
    /// status area for file work across the platform.
    file_operation_progress: Option<FileOperationProgress>,
    /// FILE-STATUS-2 — one-shot activation of the shared file-operation progress
    /// cell. The dock does not know the Files surface model; the shell drains this
    /// and opens Files on its Transfers tab.
    file_operation_progress_request: bool,
    /// NAVBAR-8 — the shell-wide interaction density mirrored from the
    /// formfactor/control-surface path. Mouse keeps the compact icon rail; Touch
    /// expands the rail into the 48px labelled variant.
    density: Density,
    /// WIN10-HYBRID — the persisted taskbar **auto-hide** setting. Off by default:
    /// the bar stays docked and reserves its bottom strut. See
    /// [`set_taskbar_autohide`](Self::set_taskbar_autohide).
    taskbar_autohide: bool,
    /// WIN10-HYBRID (B3) — the transient **reveal** latch for an auto-hidden bar:
    /// the render sets it while the pointer summons the hot edge or rides the
    /// revealed bar, and [`taskbar_reveal`] folds it with the live pointer to decide
    /// whether the retracted bar slides up this frame. Meaningless while auto-hide is
    /// off (the bar is always docked).
    taskbar_revealed: bool,
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

/// Live taskbar thumbnail snapshot keyed to one visible session entry.
#[derive(Clone)]
pub struct SessionPreviewTexture {
    id: Option<String>,
    label: String,
    protocol: &'static str,
    texture: TextureHandle,
}

impl std::fmt::Debug for SessionPreviewTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPreviewTexture")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("protocol", &self.protocol)
            .field("texture_size", &self.texture.size())
            .finish()
    }
}

impl SessionPreviewTexture {
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

    fn matches(&self, entry: &SessionRailEntry) -> bool {
        if self.id.is_some() || entry.id.is_some() {
            return self.id.as_deref() == entry.id.as_deref();
        }
        self.label == entry.label && self.protocol == entry.protocol
    }
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
        out.push_str("...");
    }
    out
}

impl DockState {
    /// Toggle the legacy **reveal** marker. The rendered left dock is retired; this
    /// survives only for compatibility regression tests that prove old reveal
    /// state cannot reserve a blank gutter.
    pub const fn toggle(&mut self) {
        self.revealed = !self.revealed;
    }

    /// Whether the legacy reveal marker says the retired dock would be shown. Kept
    /// for regression tests that prove even a "shown" legacy dock reserves no left
    /// gutter.
    #[cfg(test)]
    pub const fn shown(&self) -> bool {
        self.revealed
    }

    /// Test seam for shell-level integration fixtures: mount the NOTIF-4 detail
    /// panel in the same frame as the status bar, edge cue, and Chat surface.
    #[cfg(test)]
    pub(crate) const fn open_status_panel_for_test(&mut self) {
        self.status_panel_open = true;
    }

    /// The active surface selected by taskbar/front-door/start-menu navigation.
    /// The shell mirrors this in before the taskbar renders and reads it back out
    /// after taskbar interactions.
    pub const fn active(&self) -> Surface {
        self.active
    }

    /// Mirror the shell's live surface into the taskbar state before rendering.
    /// Hotkeys, search, and self-tests can move the surface outside the taskbar,
    /// so the read-back path must not stomp them with a stale selection.
    pub const fn set_active(&mut self, surface: Surface) {
        self.active = surface;
    }

    /// Mirror the Start Menu's ordered app pins into the application bar. This is
    /// intentionally an owned per-frame copy so taskbar clicks cannot diverge from
    /// the Start/Front Door preference store.
    pub fn set_pinned_surfaces(&mut self, pinned: &[Surface]) {
        self.pinned_surfaces.clear();
        self.pinned_surfaces.extend_from_slice(pinned);
    }

    /// Mirror active file operation progress into the dock. The Files surface owns
    /// the ledger/op reads; the dock only paints this bounded status projection.
    pub fn set_file_operation_progress(&mut self, progress: Option<FileOperationProgress>) {
        self.file_operation_progress = progress;
        self.status.segments.file_operations = self.file_operation_progress.clone();
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

    /// Refresh the bottom taskbar's live inputs (NOTIF-3) — the shell calls this
    /// each frame with the same folds the tray/status cells read
    /// (`chrome.summary()`, `system.snapshot()`, `chat.total_unread()`, the
    /// live-session flag). Owned so the taskbar's `(ctx, state)` signature stays
    /// put; cells render the pre-poll dim state until the first call lands (§7).
    /// Wired by `main.rs::mount_dock_chrome`.
    pub fn set_status_inputs(
        &mut self,
        mesh: MeshSummary,
        seat: Option<SeatSnapshot>,
        unread: usize,
        session_active: bool,
        sessions: Vec<SessionRailEntry>,
        grades: NodeGrades,
        mut segments: StatusSegments,
    ) {
        segments.file_operations = self.file_operation_progress.clone();
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

    pub(crate) fn set_session_preview(&mut self, preview: Option<SessionPreviewTexture>) {
        self.session_preview = preview;
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

    /// Compatibility no-op for the retired taskbar Start button. Launcher state is
    /// owned by the shell Front Door and clean Super tap now; the bottom taskbar
    /// no longer mirrors it into a painted cell.
    pub const fn set_start_menu_open(&mut self, _open: bool) {}

    /// Compatibility drain for the retired taskbar Start button. The taskbar no
    /// longer creates this toggle, so the drain is always empty; clean Super taps
    /// continue through `crate::hotkeys::HotkeyRouter::take_dock_toggle`.
    pub const fn take_start_menu_toggle(&mut self) -> bool {
        false
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

    /// Drain the shared file-operation progress activation (FILE-STATUS-2). Kept
    /// separate from `active == Files` so ordinary Files navigation does not force
    /// the Transfers tab.
    pub const fn take_file_operation_progress_request(&mut self) -> bool {
        let requested = self.file_operation_progress_request;
        self.file_operation_progress_request = false;
        requested
    }
}

/// Render the shell's full-width **bottom taskbar** (design
/// `docs/design/win7-desktop-survey.md`, WIN7-1 lock #3), fed the compact Desktop
/// source flyout from `ChooserState` by the shell. Left → right: the
/// **Desktop-source selector** · Start/Menu **application pins** · the
/// **running sessions** run
/// ([`SessionRailEntry`]/[`DesktopRailSource`] entries or the dim fallback glyph,
/// NAVBAR-U1/U2/U3) · the **tray** (the status-detail
/// chevron + [`status::notification_rail`]'s segment pips, unchanged
/// click-through-to-Chat behavior) · the **clock** ([`clock_cell_rect`]) · the
/// notification button and far-right show-desktop nub. Compact (`Density::Mouse`,
/// [`NOTIFICATION_RAIL_H`]) is this taskbar's default.
pub fn notification_rail_with_sources(
    ctx: &egui::Context,
    state: &mut DockState,
    desktop_sources: &[DesktopRailSource],
) -> bool {
    let screen = ctx.screen_rect();
    let rail_h = state.rail_height();
    // WIN10-HYBRID (B3) — the auto-hide slide. When the bar is docked (auto-hide
    // OFF) `reveal_t` is a hard 1.0 and `slide_off` a hard 0.0, so every rect below
    // is byte-identical to the always-docked bar (the geometry tests never take the
    // auto-hide branch). When auto-hidden the bar rides `slide_off` from fully
    // retracted (off the bottom edge) to flush, summoned by the bottom hot edge or
    // by the pointer riding the revealed bar ([`taskbar_reveal`]).
    let autohidden = state.taskbar_autohidden();
    let reveal_t = if autohidden {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let near_bottom = pointer.is_some_and(|p| p.y >= screen.bottom() - TASKBAR_HOT_EDGE_H);
        // Ride-the-bar: while the pointer is over an ALREADY-revealed bar it stays up.
        // Gated on `taskbar_revealed` so a RETRACTED bar only summons from the 4px hot
        // edge (`near_bottom`) — otherwise `over_bar` (the full 48px band) would subsume
        // the thin edge and pop the bar over content whenever the cursor neared the
        // bottom (review `autohide-reveal-band`).
        let over_bar =
            state.taskbar_revealed && pointer.is_some_and(|p| p.y >= screen.bottom() - rail_h);
        let summon = near_bottom || over_bar;
        let revealed = taskbar_reveal(true, summon, state.taskbar_revealed);
        state.taskbar_revealed = summon;
        Motion::animate(ctx, TASKBAR_REVEAL_KEY, revealed, Motion::BASE)
    } else {
        1.0
    };
    let slide_off = (1.0 - reveal_t) * rail_h;
    let rail_rect = egui::Rect::from_min_size(
        egui::pos2(screen.left(), screen.bottom() - rail_h + slide_off),
        egui::vec2(screen.width(), rail_h),
    );
    let panel_t = Motion::animate(ctx, STATUS_PANEL_KEY, state.status_panel_open, Motion::BASE);
    let panel_top = rail_rect.top() - STATUS_PANEL_GAP - STATUS_PANEL_H
        + (1.0 - panel_t.clamp(0.0, 1.0)) * Style::SP_XL;
    let mut area_top = if panel_t > 0.001 {
        panel_top.min(rail_rect.top())
    } else {
        rail_rect.top()
    };
    // Keep the Area covering at least the on-screen hot-edge sliver when the bar has
    // slid (partly) off the bottom, else `area_rect`'s height would go negative.
    if autohidden {
        area_top = area_top.min(screen.bottom() - TASKBAR_HOT_EDGE_H);
    }
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
            // content painted inside it; the rail's cells receive absolute rects
            // from `ui.allocate_exact_size`, never a separate local origin. The
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
            ui.painter()
                .rect_filled(local, egui::CornerRadius::ZERO, TASKBAR_BG);
            ui.painter().hline(
                local.left()..=local.right(),
                local.top(),
                egui::Stroke::new(HAIRLINE_W, TASKBAR_BORDER),
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

            // The far-left Start button is retired. The source caret is now the
            // first taskbar control; launching stays on the Front Door/Super paths.
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

            // The action-center button and show-desktop nub trail past the
            // sessions/tray/clock run as taskbar affordances painted below, right
            // to left.
            let tray_icon_w = rail_h.min(NOTIFICATION_RAIL_EXPANDED_ICON_H) - 4.0;
            let status_w = status::notification_rail_width(&state.status.segments, tray_icon_w);
            let clock_w = rail_h * 2.5;
            let tray_island = win11_tray_island_rect(local, rail_h, clock_w, status_w);
            let session_right = (tray_island.left() - Style::SP_XS).max(x);
            let pinned = state.pinned_surfaces.clone();
            let min_session_w = rail_h;
            let pin_right = (session_right - min_session_w - Style::SP_XS).max(x);
            for surface in pinned {
                if x + rail_h > pin_right {
                    break;
                }
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, local.top()),
                    egui::vec2(rail_h, rail_h),
                )
                .shrink(2.0);
                if pinned_app_cell(ui, rect, surface, state.active == surface) {
                    state.active = surface;
                    clicked = true;
                }
                x += rail_h + Style::SP_XS;
            }
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
                    if session_entry(
                        ui,
                        rect,
                        idx,
                        entry,
                        state.active == Surface::Desktop,
                        state.session_preview.as_ref(),
                    ) {
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
            paint_win11_tray_island(
                ui,
                tray_island,
                state.status_panel_open
                    || state.tray_overflow_open
                    || state.active == Surface::Timers
                    || state.active == Surface::Chat,
            );
            // WIN10-HYBRID #31 — the far-right **show-desktop nub**: Win10's
            // corner "minimize to desktop" sliver, a thin hairline-separated strip
            // pinned to the taskbar's very right edge.
            let nub_rect = egui::Rect::from_min_size(
                egui::pos2(local.right() - Style::SP_S, local.top()),
                egui::vec2(Style::SP_S, rail_h),
            );
            if show_desktop_nub(ui, nub_rect, state) {
                clicked = true;
            }
            let mut tray_x = local.right() - Style::SP_S - rail_h;
            // WIN10-HYBRID #31 — the **action-center** cell (Win10's tray
            // notification button): routes the body to the unified Chat feed
            // (Chat IS the notification interface here, NOTIFY-CHAT). Sits
            // immediately left of the show-desktop nub.
            if action_center_cell(ui, cell(tray_x), state) {
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
            // WIN10-HYBRID #31 — the tray-overflow cell sits immediately LEFT
            // of the status pips with its own horizontal overflow glyph.
            tray_x -= rail_h;
            let tray_overflow = cell(tray_x);
            let opened_tray_overflow = tray_overflow_toggle(ui, tray_overflow, state);
            if opened_tray_overflow {
                clicked = true;
            }
            tray_x -= rail_h;
            let status_detail = cell(tray_x);
            if status_detail_toggle(ui, status_detail, state) {
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
                if out.routed_segment == Some(StatusSegment::FileOperations) {
                    state.file_operation_progress_request = true;
                }
                clicked = true;
            }

            if panel_t > 0.001 {
                let panel_rect = notification_panel_rect(local, status_detail, panel_t);
                let mut panel_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(panel_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                panel_ui.set_opacity(panel_t.clamp(0.0, 1.0));
                let panel_out = status::status_panel(
                    &panel_ui,
                    &state.status.grades,
                    &state.status.segments,
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
                if panel_out.routed_segment == Some(StatusSegment::FileOperations) {
                    state.active = Surface::Files;
                    state.file_operation_progress_request = true;
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
                && desktop_source_flyout(ui, source_caret, desktop_sources, state)
            {
                clicked = true;
            }
            // WIN10-HYBRID #31 — the ▲ tray-overflow flyout: bottom-anchored off its
            // ▲ cell, every status segment as a reachable row (routes `active`).
            if state.tray_overflow_open
                && !opened_tray_overflow
                && tray_overflow_flyout(ui, tray_overflow, state)
            {
                clicked = true;
            }
            // WIN10-HYBRID (B3) — when auto-hidden and (partly) retracted, a thin
            // accent hot-edge sliver at the screen's true bottom hints where the bar
            // hides; it fades out as the bar slides up into view.
            if autohidden && reveal_t < 0.999 {
                let sliver = egui::Rect::from_min_size(
                    egui::pos2(screen.left(), screen.bottom() - TASKBAR_HOT_EDGE_H),
                    egui::vec2(screen.width(), TASKBAR_HOT_EDGE_H),
                );
                ui.painter().rect_filled(
                    sliver,
                    egui::CornerRadius::ZERO,
                    Style::ACCENT.linear_multiply(1.0 - reveal_t),
                );
            }
        });
    if panel_t > 0.001 && panel_t < 0.999 {
        ctx.request_repaint();
    }
    // Keep frames flowing while the auto-hide slide is in flight (a no-op once
    // settled at either end, or whenever the bar is docked).
    if autohidden && reveal_t > 0.001 && reveal_t < 0.999 {
        ctx.request_repaint();
    }
    clicked
}

/// The width of the retired left-dock gutter. Production must reserve no left
/// gutter even if legacy reveal state is toggled; the bottom taskbar strut is the
/// only live chrome reservation.
pub fn gutter_width(_ctx: &egui::Context, _state: &DockState) -> f32 {
    // WIN10-HYBRID + DEDUPE-1: the left **vertical dock** is retired — its `dock()`
    // render was deleted, so there is nothing to paint in a left gutter and it must
    // NEVER be reserved (else stale reveal/pin state would shift the whole surface
    // body 48px right behind a blank
    // column — the review `dedupe-gutter-regression`). The single 48px BOTTOM taskbar
    // (`taskbar_strut_height`) is the only chrome the shell reserves now. The
    // The stale reveal marker survives only as a harmless compatibility seam.
    0.0
}

/// WIN10-HYBRID **bottom strut** — the height the shell reserves at the bottom edge
/// for the taskbar so surface content is never covered by it (the Windows-10 model:
/// a maximized surface ends *above* the taskbar, unlike the pre-hybrid floating
/// overlay). It is the taskbar's live [`rail_height`](DockState::rail_height).
/// Unlike the retired left-dock gutter this is reserved whenever the bar is
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

/// The bottom **hot-edge** height (WIN10-HYBRID B3) — both the summon zone an
/// auto-hidden bar watches for the pointer AND the sliver painted to hint where the
/// bar hides. `SP_XS` on the 8px grid (a thin Win10-style edge).
const TASKBAR_HOT_EDGE_H: f32 = Style::SP_XS;

/// The egui memory key for the auto-hide reveal slide (the Motion latch that eases
/// the retracted bar 0↔1). Private to the taskbar.
const TASKBAR_REVEAL_KEY: &str = "taskbar-reveal";

/// Whether an auto-hidden taskbar should be **revealed** this frame (WIN10-HYBRID
/// B3) — a pure, headless decision seam. A docked bar (`autohidden == false`) is
/// always shown; an auto-hidden one reveals on an **edge summon** (the pointer at
/// the bottom hot edge) OR while it is **latched** open (riding the already-shown
/// bar). Unit-tested as a truth table without a painter.
#[must_use]
const fn taskbar_reveal(autohidden: bool, pointer_near_bottom: bool, latched: bool) -> bool {
    !autohidden || pointer_near_bottom || latched
}

/// The stable id of the Start Menu's trigger cell (WIN7-2; CONSOLE-1
/// originally), so tests read its settled `Rect`.
pub(crate) fn start_cell_id() -> egui::Id {
    egui::Id::new("vdock-start-cell")
}

/// Stable id for a bare icon-only taskbar cell ([`rail_icon`]) — keyed by the
/// glyph itself since (today) each [`IconId`] only ever backs one such cell.
fn rail_icon_id(icon: IconId) -> egui::Id {
    egui::Id::new(("bottom-rail-icon", icon.name()))
}

pub(crate) fn pinned_app_cell_id(surface: Surface) -> egui::Id {
    egui::Id::new(("bottom-rail-pinned-app", surface))
}

fn pinned_app_cell(ui: &egui::Ui, rect: egui::Rect, surface: Surface, selected: bool) -> bool {
    let id = pinned_app_cell_id(surface);
    let resp = ui.interact(rect, id, egui::Sense::click());
    let painter = ui.painter();
    if let Some(fill) = taskbar_cell_fill(selected, resp.hovered()) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected, resp.hovered(), false);
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
    if selected {
        let underline = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - ACTIVE_BAR_W),
            egui::vec2(rect.width(), ACTIVE_BAR_W),
        );
        painter.rect_filled(underline, egui::CornerRadius::ZERO, Style::ACCENT);
    }
    paint_rail_label(ui, rect, surface.label(), tint);
    paint_focus_ring(painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        id,
        surface.label(),
        if selected {
            "Active pinned application"
        } else {
            "Pinned application"
        },
        rect,
    );
    resp.clicked()
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
    _tint: egui::Color32,
    label: &str,
    value: &str,
) -> bool {
    let resp = ui.interact(rect, rail_icon_id(icon), egui::Sense::click());
    let color = taskbar_control_icon_tint(false, resp.hovered(), false);
    if let Some(fill) = taskbar_cell_fill(false, resp.hovered()) {
        ui.painter().rect_filled(rect, Style::RADIUS, fill);
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

fn session_entry_id(idx: usize, entry: &SessionRailEntry) -> egui::Id {
    egui::Id::new((
        "bottom-rail-session",
        idx,
        entry.id.as_deref(),
        entry.label.as_str(),
        entry.protocol,
    ))
}

fn session_hover_preview_id(idx: usize, entry: &SessionRailEntry) -> egui::Id {
    egui::Id::new(("bottom-rail-session-preview", session_entry_id(idx, entry)))
}

fn session_hover_protocol_badge_id(idx: usize, entry: &SessionRailEntry) -> egui::Id {
    egui::Id::new((
        "bottom-rail-session-preview-protocol",
        session_entry_id(idx, entry),
    ))
}

const SESSION_PREVIEW_W: f32 = 196.0;
const SESSION_PREVIEW_H: f32 = 124.0;
const SESSION_PREVIEW_THUMB_H: f32 = 72.0;
const SESSION_PROTOCOL_BADGE_H: f32 = 22.0;

/// WIN10-HYBRID #31 — one running-session tile is a fixed **`rail_h` square** (an
/// icons-only Win10 taskbar button); its full name rides the accesskit node, not a
/// visible caption. Kept a function taking the same `(ui, entry, rail_h)` so its
/// callers (the inline run + the overflow popup) share one width authority even
/// though it no longer measures text. `ui`/`entry` are unused now but retained for
/// that call-shape parity.
fn session_entry_width(_ui: &egui::Ui, _entry: &SessionRailEntry, rail_h: f32) -> f32 {
    rail_h
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

/// One running-session tile (WIN10-HYBRID #31) — an **icons-only** Win10 taskbar
/// button: a centred [`IconId::Sessions`] glyph over the selection wash, with a
/// Win10 **active underline** when it is the shown Desktop session. The full VM name
/// is DROPPED from the paint (the tile is a fixed [`session_entry_width`] square) but
/// still rides the accesskit node below, so a screen reader always hears it. A click
/// returns `true` (the caller routes Desktop + latches the session id).
fn session_entry(
    ui: &egui::Ui,
    rect: egui::Rect,
    idx: usize,
    entry: &SessionRailEntry,
    selected: bool,
    preview: Option<&SessionPreviewTexture>,
) -> bool {
    let resp = ui.interact(rect, session_entry_id(idx, entry), egui::Sense::click());
    let painter = ui.painter().clone();
    if let Some(fill) = taskbar_cell_fill(selected, resp.hovered()) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    if resp.hovered() {
        session_hover_preview(
            ui,
            rect,
            idx,
            entry,
            session_preview_texture_for_entry(preview, entry),
        );
    }
    let tint = taskbar_control_icon_tint(selected, resp.hovered(), false);
    let edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Sessions, edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    // The Win10 active-session **underline** — a full-width accent bar hugging the
    // tile's bottom edge (the taskbar analogue of the picker's left-edge bar).
    if selected {
        let underline = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - ACTIVE_BAR_W),
            egui::vec2(rect.width(), ACTIVE_BAR_W),
        );
        painter.rect_filled(underline, egui::CornerRadius::ZERO, Style::ACCENT);
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

/// WIN10-HYBRID #31 — static first hover thumbnail for a running Desktop session.
/// The live frame texture is a later slice; this keeps the user-visible taskbar
/// affordance in place now with the real session label and protocol badge.
fn session_hover_preview(
    ui: &egui::Ui,
    anchor: egui::Rect,
    idx: usize,
    entry: &SessionRailEntry,
    preview_texture: Option<&TextureHandle>,
) {
    let screen = ui.ctx().screen_rect();
    let margin = Style::SP_S;
    let x = (anchor.center().x - SESSION_PREVIEW_W / 2.0).clamp(
        screen.left() + margin,
        screen.right() - SESSION_PREVIEW_W - margin,
    );
    let y = anchor.top() - Style::SP_XS;
    egui::Area::new(session_hover_preview_id(idx, entry))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(x, y))
        .show(ui.ctx(), |ui| {
            let (area, _) = ui.allocate_exact_size(
                egui::vec2(SESSION_PREVIEW_W, SESSION_PREVIEW_H),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            painter.rect_filled(area, Style::RADIUS, Style::SURFACE);
            painter.rect_stroke(
                area,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );

            let thumb = egui::Rect::from_min_size(
                area.min + egui::vec2(Style::SP_S, Style::SP_S),
                egui::vec2(SESSION_PREVIEW_W - Style::SP_M, SESSION_PREVIEW_THUMB_H),
            );
            painter.rect_filled(thumb, Style::RADIUS, Style::SURFACE_HI);
            painter.rect_stroke(
                thumb,
                Style::RADIUS,
                egui::Stroke::new(1.0, Style::ACCENT.linear_multiply(0.35)),
                egui::StrokeKind::Inside,
            );
            if let Some(texture) = preview_texture {
                let image_rect = session_preview_texture_rect(texture.size(), thumb);
                let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                painter.image(texture.id(), image_rect, uv, egui::Color32::WHITE);
            } else if let Some(tex) = icon_texture(ui.ctx(), IconId::Sessions, 32.0, Style::ACCENT)
            {
                let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                let icon_rect =
                    egui::Rect::from_center_size(thumb.center(), egui::vec2(32.0, 32.0));
                painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
            }

            let title_pos = egui::pos2(
                area.left() + Style::SP_S,
                thumb.bottom() + Style::SP_XS + Style::SP_S,
            );
            let title_clip = egui::Rect::from_min_max(
                egui::pos2(area.left() + Style::SP_S, thumb.bottom() + Style::SP_XS),
                egui::pos2(area.right() - Style::SP_S, area.bottom() - Style::SP_XS),
            );
            painter.with_clip_rect(title_clip).text(
                title_pos,
                egui::Align2::LEFT_CENTER,
                entry.label.as_str(),
                egui::FontId::proportional(Style::SMALL),
                Style::TEXT,
            );

            let badge = egui::Rect::from_min_size(
                egui::pos2(
                    thumb.right() - Style::SP_XS - 52.0,
                    thumb.top() + Style::SP_XS,
                ),
                egui::vec2(52.0, SESSION_PROTOCOL_BADGE_H),
            );
            ui.interact(
                badge,
                session_hover_protocol_badge_id(idx, entry),
                egui::Sense::hover(),
            );
            painter.rect_filled(badge, Style::RADIUS, Style::ACCENT.linear_multiply(0.18));
            painter.rect_stroke(
                badge,
                Style::RADIUS,
                egui::Stroke::new(1.0, Style::ACCENT.linear_multiply(0.55)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                entry.protocol,
                egui::FontId::proportional(Style::SMALL),
                Style::ACCENT,
            );
        });
}

fn session_preview_texture_rect(size: [usize; 2], bounds: egui::Rect) -> egui::Rect {
    let width = size[0].max(1) as f32;
    let height = size[1].max(1) as f32;
    let scale = (bounds.width() / width).min(bounds.height() / height);
    egui::Rect::from_center_size(bounds.center(), egui::vec2(width * scale, height * scale))
}

fn session_preview_texture_for_entry<'a>(
    preview: Option<&'a SessionPreviewTexture>,
    entry: &SessionRailEntry,
) -> Option<&'a TextureHandle> {
    preview
        .filter(|preview| preview.matches(entry))
        .map(|preview| &preview.texture)
}

/// Stable id for the bottom-rail status detail toggle.
fn status_detail_toggle_id() -> egui::Id {
    egui::Id::new("bottom-rail-health-panel")
}

fn mesh_status_value(mesh: &MeshSummary, selected: bool) -> String {
    let panel_state = if selected { "Expanded" } else { "Collapsed" };
    if !mesh.seen {
        return format!("{panel_state}; mesh status not seen");
    }
    let health = match mesh.health {
        mde_lighthouse_health::LighthouseHealth::AllHealthy => "mesh healthy",
        mde_lighthouse_health::LighthouseHealth::Degraded => "mesh degraded",
        mde_lighthouse_health::LighthouseHealth::None => "no lighthouse status",
    };
    format!(
        "{panel_state}; {}/{} peers online; {health}",
        mesh.peers_online, mesh.peers_total
    )
}

fn status_detail_toggle(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, status_detail_toggle_id(), egui::Sense::click());
    let selected = state.status_panel_open;
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if let Some(fill) = taskbar_cell_fill(selected, hovered) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected, hovered, false);
    let edge = ICON_LOGICAL.min((rect.height() - Style::SP_S).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), STATUS_DETAIL_ICON, edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, rect, "Health", tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        status_detail_toggle_id(),
        "Health panel",
        mesh_status_value(&state.status.mesh, selected),
        rect,
    );
    if resp.clicked() {
        state.status_panel_open = !state.status_panel_open;
        return true;
    }
    false
}

/// One tray-overflow flyout row's height — a compact `SP_L` line, dot + label.
const TRAY_OVERFLOW_ROW_H: f32 = Style::SP_L;
/// The tray-overflow flyout's fixed width — wide enough for a segment name + dot.
const TRAY_OVERFLOW_W: f32 = Style::SP_XL * 4.0;

/// Stable id for the tray-overflow toggle cell (WIN10-HYBRID #31) — its own tag,
/// so it never collides with [`status_detail_toggle_id`]'s health cell.
fn tray_overflow_id() -> egui::Id {
    egui::Id::new("bottom-rail-tray-overflow")
}

/// Stable id for the tray-overflow flyout's floating `Area`.
fn tray_overflow_popup_id() -> egui::Id {
    egui::Id::new("bottom-rail-tray-overflow-popup")
}

/// Stable id for one tray-overflow flyout row, addressable per segment.
fn tray_overflow_row_id(segment: StatusSegment) -> egui::Id {
    egui::Id::new(("bottom-rail-tray-overflow-row", segment))
}

const fn tray_segment_label(segment: StatusSegment) -> &'static str {
    status::segment_label(segment)
}

/// The surface a tray-overflow row routes to (mirrors `status.rs`'s segment
/// routing; see [`tray_segment_label`]).
const fn tray_segment_route(segment: StatusSegment) -> Surface {
    match segment {
        StatusSegment::Device | StatusSegment::Power | StatusSegment::RemoteControl => {
            Surface::System
        }
        StatusSegment::Mesh => Surface::MeshView,
        StatusSegment::FileOperations => Surface::Files,
        StatusSegment::Alerts => Surface::Chat,
    }
}

/// The **tray-overflow** cell (WIN10-HYBRID #31) — a distinct horizontal overflow
/// glyph immediately left of the status pips. A click toggles the bottom-anchored
/// flyout of every status segment ([`tray_overflow_flyout`]).
fn tray_overflow_toggle(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, tray_overflow_id(), egui::Sense::click());
    let selected = state.tray_overflow_open;
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if let Some(fill) = taskbar_cell_fill(selected, hovered) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected, hovered, false);
    let edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), TRAY_OVERFLOW_ICON, edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    paint_rail_label(ui, rect, "Tray", tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        tray_overflow_id(),
        "Notifications overflow",
        if selected { "Expanded" } else { "Collapsed" },
        rect,
    );
    if response_activated(ui, &resp) {
        state.tray_overflow_open = !state.tray_overflow_open;
        return true;
    }
    false
}

/// The tray-overflow **flyout** (WIN10-HYBRID #31) — a bottom-anchored popup off the
/// ▲ cell listing every [`StatusSegment::ALL`] entry as a reachable row (a severity
/// dot + the segment name); a row click routes [`DockState::active`] to that
/// segment's surface and closes the flyout, and a click-away dismisses it (the
/// `rail_more_popup` / `desktop_source_flyout` idiom). Returns `true` when it routed
/// or dismissed this frame.
fn tray_overflow_flyout(ui: &egui::Ui, anchor: egui::Rect, state: &mut DockState) -> bool {
    let rows = StatusSegment::ALL.len();
    let popup_h = rows as f32 * TRAY_OVERFLOW_ROW_H + Style::SP_S;
    let popup_pos = egui::pos2(anchor.left(), anchor.top() - Style::SP_XS);
    let popup_screen_rect = egui::Rect::from_min_size(
        egui::pos2(popup_pos.x, popup_pos.y - popup_h),
        egui::vec2(TRAY_OVERFLOW_W, popup_h),
    );
    let segments = state.status.segments.clone();
    let inner = egui::Area::new(tray_overflow_popup_id())
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(popup_pos)
        .show(ui.ctx(), |ui| {
            let (area, _) =
                ui.allocate_exact_size(egui::vec2(TRAY_OVERFLOW_W, popup_h), egui::Sense::hover());
            let bg = ui.painter().add(egui::Shape::Noop);
            let mut routed: Option<StatusSegment> = None;
            let mut y = area.top() + Style::SP_XS / 2.0;
            for segment in StatusSegment::ALL {
                let row = egui::Rect::from_min_size(
                    egui::pos2(area.left() + Style::SP_XS, y),
                    egui::vec2(TRAY_OVERFLOW_W - Style::SP_S, TRAY_OVERFLOW_ROW_H),
                );
                if tray_overflow_row(ui, row, segment, &segments) {
                    routed = Some(segment);
                }
                y += TRAY_OVERFLOW_ROW_H;
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
            routed
        });
    if let Some(segment) = inner.inner {
        state.active = tray_segment_route(segment);
        if segment == StatusSegment::FileOperations {
            state.file_operation_progress_request = true;
        }
        state.tray_overflow_open = false;
        return true;
    }
    let pointer_inside_popup = ui.ctx().input(|i| {
        i.pointer
            .latest_pos()
            .is_some_and(|p| popup_screen_rect.contains(p))
    });
    if inner.response.clicked_elsewhere() && !pointer_inside_popup {
        state.tray_overflow_open = false;
        return true;
    }
    false
}

/// One tray-overflow flyout row: a severity dot (the SAME `status.rs` severity fold
/// the tray pips read) + the segment name. Hover fills only. Returns `true` on a
/// click or keyboard activation (the caller routes `active`).
fn tray_overflow_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    segment: StatusSegment,
    segments: &StatusSegments,
) -> bool {
    let resp = ui.interact(rect, tray_overflow_row_id(segment), egui::Sense::click());
    let painter = ui.painter().clone();
    if resp.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let dot_c = egui::pos2(rect.left() + Style::SP_S, rect.center().y);
    painter.circle_filled(
        dot_c,
        Style::SP_XS,
        status::segment_color(segment, segments),
    );
    painter.text(
        egui::pos2(dot_c.x + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        tray_segment_label(segment),
        egui::FontId::proportional(Style::SMALL),
        if resp.hovered() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        },
    );
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        tray_overflow_row_id(segment),
        tray_segment_label(segment),
        status::segment_accessibility_value(segment, segments),
        rect,
    );
    response_activated(ui, &resp)
}

/// Stable id for the taskbar **action-center** cell (WIN10-HYBRID #31) — the
/// Win10 tray "action center" affordance. Keyed by its own tag so it never
/// collides with the bare [`rail_icon`] Chat glyph.
fn action_center_cell_id() -> egui::Id {
    egui::Id::new(("bottom-rail-action-center", ACTION_CENTER_ICON.name()))
}

/// The taskbar **action-center** cell (WIN10-HYBRID #31) — Win10's tray
/// notification button. Chat IS this shell's unified notification feed
/// (NOTIFY-CHAT: every mesh host is a contact, its alerts are its messages), so a
/// click routes the body to [`Surface::Chat`]. It wears the ACCENT tint when Chat
/// is already the active surface OR there are unread events (the "you have
/// notifications" cue), and otherwise follows the shared tray-cell hover/rest
/// idiom. Every colour is a [`Style`] token (§4). Returns `true` on a click.
fn action_center_cell(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, action_center_cell_id(), egui::Sense::click());
    let selected = state.active == Surface::Chat;
    let unread = state.status.unread > 0;
    let hovered = resp.hovered();
    let painter = ui.painter().clone();
    if let Some(fill) = taskbar_cell_fill(selected, hovered) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected || unread, hovered, false);
    let edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), ACTION_CENTER_ICON, edge, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(
            tex.id(),
            rail_icon_rect(rect, edge),
            uv,
            egui::Color32::WHITE,
        );
    }
    if unread {
        painter.circle_filled(
            egui::pos2(rect.right() - Style::SP_S, rect.top() + Style::SP_S),
            3.0,
            Style::ACCENT,
        );
    }
    paint_rail_label(ui, rect, "Notifications", tint);
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        action_center_cell_id(),
        "Action center",
        if unread {
            "Unread notifications"
        } else {
            "No unread notifications"
        },
        rect,
    );
    if resp.clicked() {
        state.active = Surface::Chat;
        return true;
    }
    false
}

/// Stable id for the taskbar **show-desktop nub** (WIN10-HYBRID #31).
fn show_desktop_nub_id() -> egui::Id {
    egui::Id::new("bottom-rail-show-desktop")
}

/// The **show-desktop nub** (WIN10-HYBRID #31) — Win10's far-right-corner "show
/// desktop" sliver: a thin ([`Style::SP_S`]-wide) hairline-separated vertical
/// strip pinned to the taskbar's very right edge. A click minimizes to the
/// desktop by routing the body to the [`Surface::Desktop`] VDI surface (this
/// shell's "show desktop" target). The hairline gives it the Win10 corner
/// separator; a subtle hover wash gives it presence. Every colour is a [`Style`]
/// token (§4). Returns `true` on a click.
fn show_desktop_nub(ui: &egui::Ui, rect: egui::Rect, state: &mut DockState) -> bool {
    let resp = ui.interact(rect, show_desktop_nub_id(), egui::Sense::click());
    let painter = ui.painter().clone();
    if resp.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, TASKBAR_HOVER_FILL);
    }
    // The Win10 hairline that fences the show-desktop corner off from the tray.
    painter.vline(
        rect.left(),
        rect.top()..=rect.bottom(),
        egui::Stroke::new(HAIRLINE_W, TASKBAR_BORDER),
    );
    paint_focus_ring(&painter, rect, resp.has_focus());
    install_cell_accessibility(
        ui.ctx(),
        show_desktop_nub_id(),
        "Show desktop",
        if state.active == Surface::Desktop {
            "Desktop shown"
        } else {
            "Minimize to desktop"
        },
        rect,
    );
    if resp.clicked() {
        state.active = Surface::Desktop;
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
    if let Some(fill) = taskbar_cell_fill(selected, hovered) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected, hovered, false);
    let now = crate::timers::now_unix();
    let time_text = crate::timers::hhmm(now);
    let time_font =
        egui::FontId::proportional((Style::SMALL + 1.0).min((rect.height() - 6.0).max(10.0)));
    if rect.height() >= NOTIFICATION_RAIL_EXPANDED_H - 1.0 {
        // Windows 11-style tray clock: compact, right-aligned HH:MM over the date.
        // The crate's ONE calendar (`chat::civil_from_days`) formats it so no
        // second date fold leaks in.
        let text_x = rect.right() - Style::SP_S;
        painter.text(
            egui::pos2(text_x, rect.center().y - Style::SP_XS - 1.0),
            egui::Align2::RIGHT_CENTER,
            &time_text,
            time_font,
            tint,
        );
        painter.text(
            egui::pos2(text_x, rect.center().y + Style::SP_S),
            egui::Align2::RIGHT_CENTER,
            &clock_date_text(now),
            egui::FontId::proportional(Style::SMALL - 1.0),
            TASKBAR_CLOCK_DATE,
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
// one fix covers both — the status/notification toggle, the clock, the
// action-center cell, and the show-desktop nub). Restates the SAME
// `accesskit_rect` helper + `Role::Button` +
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

/// The active cell's **left-edge accent bar** (lock #10) — an `SP_XS`-wide
/// [`Style::ACCENT`] bar down the active surface's left edge (the vertical analog
/// of the horizontal bar's bottom underline), at the cell's outer edge.
const ACTIVE_BAR_W: f32 = Style::SP_XS;

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
const DESKTOP_CARET_W: f32 = NOTIFICATION_RAIL_H;
const DESKTOP_SOURCE_ROW_H: f32 = 28.0;
const DESKTOP_SOURCE_FLYOUT_W: f32 = Style::SP_XL * 7.5;
const DESKTOP_SOURCE_MAX_ROWS: usize = 8;

/// NOTIF-4's right slide-out width: compact enough to stay auxiliary, wide enough
/// for grade names and three device meters.
const STATUS_PANEL_W: f32 = Style::SP_XL * 7.0;
const STATUS_PANEL_GAP: f32 = Style::SP_XS;
const STATUS_PANEL_H: f32 = Style::SP_XL * 8.0;

fn notification_panel_rect(rail: egui::Rect, anchor: egui::Rect, t: f32) -> egui::Rect {
    let min_left = rail.left() + Style::SP_S;
    let max_left = (rail.right() - Style::SP_S - STATUS_PANEL_W).max(min_left);
    let left = (anchor.right() - STATUS_PANEL_W).clamp(min_left, max_left);
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
    if let Some(fill) = taskbar_cell_fill(selected, hovered) {
        painter.rect_filled(rect, Style::RADIUS, fill);
    }
    let tint = taskbar_control_icon_tint(selected, hovered, empty);
    let edge = (rect.height() - 2.0).max(Style::SP_S);
    if let Some(tex) = icon_texture(ui.ctx(), DESKTOP_SOURCE_TOGGLE_ICON, edge, tint) {
        let icon = rail_icon_rect(rect, edge);
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

fn rail_more_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    hidden_count: usize,
) -> bool {
    let resp = ui.interact(rect, rail_more_id(), egui::Sense::click());
    let active = state.rail_more_open || resp.hovered();
    if active {
        ui.painter().rect_filled(
            rect,
            Style::RADIUS,
            taskbar_cell_fill(state.rail_more_open, resp.hovered()).unwrap_or(TASKBAR_HOVER_FILL),
        );
    }
    let tint = taskbar_control_icon_tint(state.rail_more_open, resp.hovered(), false);
    let edge = ICON_LOGICAL.min((rect.height() - 4.0).max(Style::SP_S));
    if let Some(tex) = icon_texture(ui.ctx(), IconId::MoreHorizontal, edge, tint) {
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(edge, edge));
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }
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
                if session_entry(
                    ui,
                    rect,
                    idx,
                    entry,
                    state.active == Surface::Desktop,
                    state.session_preview.as_ref(),
                ) {
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

/// Keyboard-focus-ring stroke width (a11y-03 / WCAG 2.4.7) — the shared platform
/// **2px** focus token ([`mde_egui::focus::FOCUS_RING_W`], design lock #5), so every
/// focus indicator across the shell (taskbar cells, Explorer, Console) reads at one
/// consistent weight against the dark platform ground. Sourced from the one token
/// rather than the old mirrored `2.5` local literals.
const FOCUS_RING_W: f32 = mde_egui::focus::FOCUS_RING_W;

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
/// The platform palette has no dedicated focus token, so the ring wears the lifted
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

#[cfg(test)]
mod tests;
