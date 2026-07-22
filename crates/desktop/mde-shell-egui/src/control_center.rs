//! `control_center` — WL-UX-006/U12: the **Construct Control Center** — the
//! top-right tile overlay that replaces every tray flyout the Win10 chrome had
//! (volume · display/brightness · network/mesh · Bluetooth · Construct↔Car ·
//! VDI sessions · file-operation progress), over a click-to-dismiss scrim.
//!
//! Authority: `docs/design/platform-interfaces.md` §2.4 (Q13). The U09
//! scaffold reserved the `mount_control_center_slot` mount point and the
//! [`ConstructChrome::control_center_open`] flag; this module is the surface
//! that lands into it — `main.rs` only forwards the seams (U09's whole point).
//!
//! ## Honest data, real seams (§6/§7 — zero dead tiles)
//!
//! Every tile controls, navigates, or displays LIVE state — never a mockup:
//!
//! * **Volume** — reads the seat snapshot's mixer probe and steps/mutes through
//!   the SAME `SystemState::dispatch_hotkey` seam the hardware volume keys
//!   drive (one mixer choke point, §6). Mixer `Absent` → an Audio-settings
//!   deep-link tile, never a slider over nothing.
//! * **Brightness** — shown only when the seat reports a controllable device
//!   (a sysfs backlight or a DDC/CI monitor); steps ride the same hotkey seam.
//!   No device → a Displays deep-link tile instead (the design's "do NOT
//!   fabricate a slider" rule).
//! * **Network/mesh** — the mesh peers-online summary + the notify worker's
//!   Mesh segment rollup (the exact StatusSegments the tray pips read); the
//!   tile deep-links to Settings → Network.
//! * **Bluetooth** — the snapshot's BlueZ probe (radio power + connected
//!   count); a click toggles the first adapter's radio through the hotkey
//!   seam. No adapter → an honest deep-link to Settings → Bluetooth (where the
//!   E12-17 `bt_pairing` responder lives).
//! * **Construct↔Car** — the SAME `layout_mode_primary_toggle` +
//!   `SystemState::set_layout_profile` seam the floating mode button drives
//!   (never a second toggle path); entering Car lands on the Auto-Mode home,
//!   mirroring `apply_layout_profile` (the vdock density mirror self-heals on
//!   the next `mount_dock_chrome` frame).
//! * **VDI sessions** — the `SessionRailState` projection of the broker's
//!   public session log, with a **focus** action per row (the taskbar's
//!   `focus_session` seam + the Desktop route). A **disconnect** action is
//!   deliberately NOT shipped: the shell has no honest disconnect seam — the
//!   rail is a read-projection and publishing a fake broker `Disconnect`
//!   transition would lie about lifecycle the integration-gated `SessionStore`
//!   owns. No sessions → a Remote-Sessions deep-link tile.
//! * **File operations** — the same Files/Browser `OperationProgressSummary`
//!   fold the taskbar status cell mirrors (`shell_file_operation_progress`'s
//!   Files-first precedence); the row exists only while jobs are active and
//!   routes through the SAME `route_file_operation_progress_request` seam.
//!
//! Deep-links need no U29 wiring: the mount slot forwards `nav` + the System
//! state directly, so navigation lands through the same fields every other
//! nav path writes.

use mde_egui::egui;
use mde_egui::motion::Spring;
use mde_egui::{paint_carbon, LayoutProfile, Motion, OsdLevel, Style};
use mde_files_egui::model::OperationProgressSummary;
use mde_files_egui::FileBrowser;
use mde_seat::hotkeys::HotkeyAction;
use mde_seat::{Probe, SeatSnapshot};
use mde_theme::brand::icons::IconId;

use crate::chrome::MeshSummary;
use crate::construct::{ChromeIntent, ConstructChrome};
use crate::dock::{icon_texture, SessionRailEntry, Surface};
use crate::session_rail::SessionRailState;
use crate::status::{severity_color, StatusSegments};
use crate::system::{SettingsSection, SystemState};
use crate::web::WebState;
use crate::Nav;

// ──────────────────────────── geometry ────────────────────────────

/// The card's fixed width — two tile columns plus padding, matching the
/// `LAYOUT_MODE_MENU_W` bare-const idiom for chrome panel dims.
const PANEL_W: f32 = 380.0;
/// Screen-edge margin the card floats off the top-right corner by.
const PANEL_MARGIN: f32 = Style::SP_M;
/// Card inner padding.
const PANEL_PAD: f32 = Style::SP_M;
/// A full-width control row (volume / brightness / file operations).
const ROW_H: f32 = 44.0;
/// One grid tile (RADIUS_L card cell).
const TILE_H: f32 = 64.0;
/// One VDI session row.
const SESSION_ROW_H: f32 = 36.0;
/// The footnote header above the session list.
const HEADER_H: f32 = 18.0;
/// Gap between grid cells / list rows.
const GRID_GAP: f32 = Style::SP_S;
/// Gap between the card's sections.
const SECTION_GAP: f32 = Style::SP_M;
/// Sessions shown before the header's total count carries the rest — keeps the
/// overlay glanceable (the taskbar rail remains the full list).
const MAX_SESSION_ROWS: usize = 4;
/// Below this reveal fraction a closing panel stops painting entirely.
const REVEAL_EPSILON: f32 = 0.01;

/// The spring state key for the entry/exit reveal (`Motion::spring_to` — the
/// macOS-style settle; reduce-motion collapses it to the endpoint instantly).
const REVEAL_KEY: &str = "shell-control-center-reveal";
/// The overlay `Area` id.
const AREA_KEY: &str = "shell-control-center";
/// The card-background click swallow (so a click inside the card that misses a
/// control never reads as a scrim dismiss).
const PANEL_BG_KEY: &str = "shell-control-center-bg";
/// The ctx-memory key holding the last seat-acknowledged brightness fraction —
/// the honest post-nudge display value (the write-back the seat confirmed),
/// bridging the 5 s snapshot cadence exactly like the OSD flash does.
const BRIGHTNESS_KEY: &str = "shell-control-center-brightness";

// ──────────────────────────── the model (pure folds) ────────────────────────────

/// The volume row's live reading, folded from the seat mixer probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VolumeModel {
    /// Master volume 0–100 (the cached strip `dispatch_hotkey` accumulates on).
    volume: u8,
    /// Master mute.
    muted: bool,
}

/// The mixer probe → the volume row, or `None` when the seat has no mixer
/// (the honest absent — the grid then carries an Audio deep-link tile).
fn volume_model(snap: Option<&SeatSnapshot>) -> Option<VolumeModel> {
    match &snap?.mixer {
        Probe::Present(m) => Some(VolumeModel {
            volume: m.master.volume,
            muted: m.master.muted,
        }),
        Probe::Absent { .. } => None,
    }
}

/// The first controllable brightness device's current percent: an internal
/// sysfs backlight panel first, else a DDC/CI monitor — the SAME preference
/// order `dispatch_hotkey`'s brightness nudge acts in, so the row always shows
/// the device the buttons will drive. `None` = nothing controllable.
fn brightness_seed(snap: Option<&SeatSnapshot>) -> Option<u8> {
    let snap = snap?;
    if let Probe::Present(panels) = &snap.backlights {
        if let Some(panel) = panels.first() {
            return Some(panel.percent());
        }
    }
    if let Probe::Present(ddc) = &snap.ddc {
        if let Some(monitor) = ddc.first() {
            return Some(monitor.brightness);
        }
    }
    None
}

/// The Bluetooth tile's live reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BtModel {
    /// An adapter answered the probe.
    Present {
        /// Any adapter radio is powered.
        powered: bool,
        /// Currently-connected device count.
        connected: usize,
    },
    /// No adapter / probe absent — the tile becomes a settings deep-link.
    Absent,
}

/// The BlueZ probe → the Bluetooth tile state.
fn bluetooth_model(snap: Option<&SeatSnapshot>) -> BtModel {
    match snap.map(|s| &s.bluetooth) {
        Some(Probe::Present(bt)) if !bt.adapters.is_empty() => BtModel::Present {
            powered: bt.any_adapter_powered(),
            connected: bt.connected_devices(),
        },
        _ => BtModel::Absent,
    }
}

/// One VDI session row (a read-projection of [`SessionRailEntry`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRow {
    /// The broker session id, when the row can be focused.
    id: Option<String>,
    /// The bounded human label.
    label: String,
    /// The `VDI` / `LIVE` / `DISC` badge.
    badge: &'static str,
}

/// The rail entries → glanceable rows (capped at [`MAX_SESSION_ROWS`]).
fn session_rows(entries: &[SessionRailEntry]) -> Vec<SessionRow> {
    entries
        .iter()
        .take(MAX_SESSION_ROWS)
        .map(|e| SessionRow {
            id: e.session_id().map(str::to_owned),
            label: e.label().to_owned(),
            badge: e.protocol(),
        })
        .collect()
}

/// The file-operation progress row, present only while jobs are active.
#[derive(Debug, Clone, PartialEq)]
struct FileOpModel {
    /// Active job count.
    active: usize,
    /// Average known progress; `None` while every job is still starting (the
    /// row then shows an honest "starting", never a fabricated percent).
    fraction: Option<f32>,
    /// The bounded primary label.
    label: String,
}

/// Files-first, Browser-fallback — the SAME precedence
/// `shell_file_operation_progress` feeds the taskbar status cell — gated on
/// `active > 0` so an idle summary never paints a dead row.
fn file_ops_model(
    files: Option<OperationProgressSummary>,
    browser: Option<OperationProgressSummary>,
) -> Option<FileOpModel> {
    let summary = files.or(browser)?;
    (summary.active > 0).then(|| FileOpModel {
        active: summary.active,
        fraction: summary.fraction,
        label: summary.label,
    })
}

/// A tile's glyph: a Mackes-Carbon registry name where one exists
/// ([`paint_carbon`]), else the YAMIS [`IconId`] fallback the shell chrome
/// already rasterizes ([`icon_texture`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Glyph {
    /// An embedded Mackes-Carbon glyph name.
    Carbon(&'static str),
    /// A YAMIS brand icon.
    Icon(IconId),
}

/// Everything a Control Center action can DO — each variant is a real control
/// mutation or a real route (§7: zero dead tiles is enforced by this type).
#[derive(Debug, Clone, PartialEq, Eq)]
enum CcAction {
    /// Master volume −5 through the hotkey seam.
    VolumeDown,
    /// Master volume +5 through the hotkey seam.
    VolumeUp,
    /// Master mute toggle through the hotkey seam.
    VolumeMuteToggle,
    /// Brightness −5 (backlight-first, then DDC) through the hotkey seam.
    BrightnessDown,
    /// Brightness +5 through the hotkey seam.
    BrightnessUp,
    /// Toggle the first Bluetooth adapter's radio through the hotkey seam.
    BluetoothToggle,
    /// The Construct↔Car primary toggle (`layout_mode_primary_toggle`).
    LayoutToggle,
    /// Deep-link: open Settings resting on `section`.
    OpenSettings(SettingsSection),
    /// Deep-link: open the Remote Sessions (Desktop) surface.
    OpenDesktop,
    /// Focus a broker-visible VDI session and show the Desktop surface.
    FocusSession(String),
    /// Route to Files → Transfers (the taskbar's file-operations jump).
    OpenFileOperations,
}

/// One grid tile: a glyph, two text lines, an optional status tone dot, and
/// the real action a click performs.
#[derive(Debug, Clone, PartialEq)]
struct Tile {
    /// The action glyph.
    glyph: Glyph,
    /// The `TYPE_FOOTNOTE` title line.
    title: &'static str,
    /// The dimmed caption line (live state or the route's description).
    caption: String,
    /// Accent-stroked (an engaged toggle: BT radio on, Car mode active).
    active: bool,
    /// A status dot tint (the network tile's segment severity), when any.
    tone: Option<egui::Color32>,
    /// What a click does.
    action: CcAction,
}

/// The whole card, folded once per frame from the live states — pure over its
/// inputs so composition (which tiles exist, §7) is unit-tested headless.
#[derive(Debug, Clone, PartialEq)]
struct PanelModel {
    /// The volume row (mixer Present).
    volume: Option<VolumeModel>,
    /// The brightness row's percent (a controllable device exists).
    brightness: Option<u8>,
    /// The 2-column tile grid.
    tiles: Vec<Tile>,
    /// The glanceable VDI session rows.
    sessions: Vec<SessionRow>,
    /// Total broker-visible sessions (the header count when capped).
    sessions_total: usize,
    /// The file-operation progress row.
    file_ops: Option<FileOpModel>,
}

/// Fold the card from the live states. `brightness_override` is the last
/// seat-acknowledged nudge value ([`BRIGHTNESS_KEY`]), preferred over the
/// snapshot seed between the pump's 5 s republishes.
#[allow(clippy::too_many_arguments)] // the fold IS the one place the seams meet
fn fold_model(
    snap: Option<&SeatSnapshot>,
    brightness_override: Option<f32>,
    profile: LayoutProfile,
    mesh: &MeshSummary,
    segments: &StatusSegments,
    entries: &[SessionRailEntry],
    files: Option<OperationProgressSummary>,
    browser: Option<OperationProgressSummary>,
) -> PanelModel {
    let volume = volume_model(snap);
    let brightness = brightness_seed(snap).map(|seed| {
        brightness_override.map_or(seed, |frac| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            // clamped 0..=1 → 0..=100 fits u8
            {
                (frac.clamp(0.0, 1.0) * 100.0).round() as u8
            }
        })
    });
    let sessions = session_rows(entries);
    let file_ops = file_ops_model(files, browser);

    let mut tiles = vec![
        network_tile(mesh, segments),
        bluetooth_tile(snap),
        layout_tile(profile),
    ];
    if brightness.is_none() {
        // No controllable device → never a fabricated slider; a real route.
        tiles.push(Tile {
            glyph: Glyph::Icon(IconId::DisplaySettings),
            title: "Displays",
            caption: "Open display settings".to_owned(),
            active: false,
            tone: None,
            action: CcAction::OpenSettings(SettingsSection::Displays),
        });
    }
    if volume.is_none() {
        // Mixer Absent → the honest Audio-settings route.
        tiles.push(Tile {
            glyph: Glyph::Carbon("audio-volume-muted"),
            title: "Audio",
            caption: "No mixer - open settings".to_owned(),
            active: false,
            tone: None,
            action: CcAction::OpenSettings(SettingsSection::Audio),
        });
    }
    if sessions.is_empty() {
        tiles.push(Tile {
            glyph: Glyph::Icon(IconId::Workstation),
            title: "Sessions",
            caption: "Open Remote Sessions".to_owned(),
            active: false,
            tone: None,
            action: CcAction::OpenDesktop,
        });
    }

    PanelModel {
        volume,
        brightness,
        tiles,
        sessions,
        sessions_total: entries.len(),
        file_ops,
    }
}

/// The Network/mesh tile: peers-online from the mesh summary, tinted by the
/// notify worker's Mesh segment severity (the tray pips' exact sources).
fn network_tile(mesh: &MeshSummary, segments: &StatusSegments) -> Tile {
    let caption = if mesh.seen {
        format!("{}/{} peers online", mesh.peers_online, mesh.peers_total)
    } else {
        // The honest pre-read state — dim words, never invented counts.
        "Mesh not read yet".to_owned()
    };
    Tile {
        glyph: Glyph::Carbon("globe"),
        title: "Network",
        caption,
        active: false,
        tone: Some(severity_color(segments.mesh.as_ref())),
        action: CcAction::OpenSettings(SettingsSection::Network),
    }
}

/// The Bluetooth tile: live radio/connection state with the toggle action, or
/// the honest settings deep-link when no adapter answered.
fn bluetooth_tile(snap: Option<&SeatSnapshot>) -> Tile {
    match bluetooth_model(snap) {
        BtModel::Present { powered, connected } => Tile {
            glyph: Glyph::Icon(IconId::Bluetooth),
            title: "Bluetooth",
            caption: if powered {
                format!("On - {connected} connected")
            } else {
                "Off".to_owned()
            },
            active: powered,
            tone: None,
            action: CcAction::BluetoothToggle,
        },
        BtModel::Absent => Tile {
            glyph: Glyph::Icon(IconId::Bluetooth),
            title: "Bluetooth",
            caption: "No adapter - open settings".to_owned(),
            active: false,
            tone: None,
            action: CcAction::OpenSettings(SettingsSection::Bluetooth),
        },
    }
}

/// The Construct↔Car tile — the mode toggle's overlay twin.
fn layout_tile(profile: LayoutProfile) -> Tile {
    Tile {
        glyph: Glyph::Carbon("view-refresh"),
        title: "Mode",
        caption: profile.label().to_owned(),
        active: profile.is_car(),
        tone: None,
        action: CcAction::LayoutToggle,
    }
}

impl PanelModel {
    /// The sections present, in paint order (drives the height + gap math).
    fn section_count(&self) -> usize {
        usize::from(self.volume.is_some())
            + usize::from(self.brightness.is_some())
            + usize::from(!self.tiles.is_empty())
            + usize::from(!self.sessions.is_empty())
            + usize::from(self.file_ops.is_some())
    }

    /// The card height for this frame's content.
    #[allow(clippy::cast_precision_loss)] // row counts are tiny
    fn height(&self) -> f32 {
        let mut h = PANEL_PAD * 2.0;
        if self.volume.is_some() {
            h += ROW_H;
        }
        if self.brightness.is_some() {
            h += ROW_H;
        }
        if !self.tiles.is_empty() {
            let rows = self.tiles.len().div_ceil(2);
            h += rows as f32 * TILE_H + rows.saturating_sub(1) as f32 * GRID_GAP;
        }
        if !self.sessions.is_empty() {
            let rows = self.sessions.len();
            h += HEADER_H + rows as f32 * SESSION_ROW_H + rows.saturating_sub(1) as f32 * GRID_GAP;
        }
        if self.file_ops.is_some() {
            h += ROW_H;
        }
        h + self.section_count().saturating_sub(1) as f32 * SECTION_GAP
    }
}

// ──────────────────────────── the mount seam ────────────────────────────

/// Everything the U09 slot forwards — the shell's real seams, nothing copied.
pub(crate) struct ControlCenterDeps<'a> {
    /// The Construct chrome (intent queue + the open flag this module owns).
    pub construct: &'a mut ConstructChrome,
    /// The ONE seat state: mixer/brightness/BT verbs + the layout profile.
    pub system: &'a mut SystemState,
    /// The shell nav — deep-links land on the same fields every path writes.
    pub nav: &'a mut Nav,
    /// The VDI session rail projection (list + focus).
    pub session_rail: &'a mut SessionRailState,
    /// Files — operation summaries + the Transfers route.
    pub files: &'a mut FileBrowser,
    /// Browser — the downloads half of the operation summary.
    pub web: &'a mut WebState,
    /// The tray's live mesh summary (peers/health), read-only.
    pub mesh: &'a MeshSummary,
    /// The notify worker's segment rollups, read-only.
    pub segments: &'a StatusSegments,
    /// This seat's hostname (the session rail's client filter).
    pub local_host: &'a str,
}

/// Mount the Control Center for this frame: consume the queued
/// [`ChromeIntent::ControlCenter`], run the dismiss edges (Escape + scrim
/// click), spring the reveal, and paint the card when visible.
pub(crate) fn mount(ctx: &egui::Context, mut deps: ControlCenterDeps<'_>) {
    if deps.construct.take_intent(ChromeIntent::ControlCenter) {
        deps.construct.control_center_open = !deps.construct.control_center_open;
    }
    if deps.construct.control_center_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        deps.construct.control_center_open = false;
    }
    let open = deps.construct.control_center_open;
    // Entry/exit ride the shared chrome spring; reduce-motion returns the
    // endpoint immediately (`Motion::spring_to`), so the overlay pops instantly.
    let reveal = Motion::spring_to(
        ctx,
        egui::Id::new(REVEAL_KEY),
        if open { 1.0 } else { 0.0 },
        Spring::SNAPPY,
    );
    if !open && reveal <= REVEAL_EPSILON {
        return;
    }

    // Live inputs for this frame's fold. The dock chrome already pumped these
    // in Construct mode; re-pumping drains nothing new and keeps the overlay
    // live in Car mode, where the taskbar (and its pump) is hidden.
    deps.files.pump_transfers();
    deps.web.pump_downloads_for_shell_chrome();
    let entries = deps.session_rail.entries(deps.local_host);
    let model = fold_model(
        deps.system.snapshot(),
        stored_brightness(ctx),
        deps.system.layout_profile(),
        deps.mesh,
        deps.segments,
        &entries,
        deps.files.operation_progress_summary(),
        deps.web.operation_progress_summary(),
    );

    let screen = ctx.screen_rect();
    let panel = panel_rect(screen, model.height(), reveal);
    let mut action: Option<CcAction> = None;
    let mut scrim_clicked = false;
    egui::Area::new(egui::Id::new(AREA_KEY))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .show(ctx, |ui| {
            // The SCRIM_THIN backdrop claims the whole screen (the curtain's
            // whole-screen-claim idiom), so egui routes every outside click
            // here — and the card's own widgets, registered after, still win
            // inside their rects.
            let (claim_rect, claim) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(claim_rect, 0.0, Style::SCRIM_THIN.gamma_multiply(reveal));
            scrim_clicked = claim.clicked();
            action = paint_panel(ui, panel, &model);
        });
    if scrim_clicked {
        deps.construct.control_center_open = false;
    }
    if let Some(action) = action {
        apply(&action, ctx, &mut deps);
    }
}

/// The card rect: anchored to the top-right, slid up off-screen by the
/// un-revealed remainder (the drop-from-the-corner entry).
fn panel_rect(screen: egui::Rect, height: f32, reveal: f32) -> egui::Rect {
    let rest = egui::Rect::from_min_size(
        egui::pos2(
            screen.right() - PANEL_MARGIN - PANEL_W,
            screen.top() + PANEL_MARGIN,
        ),
        egui::vec2(PANEL_W, height),
    );
    rest.translate(egui::vec2(
        0.0,
        -(1.0 - reveal) * (height + PANEL_MARGIN * 2.0),
    ))
}

// ──────────────────────────── the action drain ────────────────────────────

/// Apply one clicked action through the real seams. Navigation actions also
/// dismiss the overlay so the route is immediately visible.
fn apply(action: &CcAction, ctx: &egui::Context, deps: &mut ControlCenterDeps<'_>) {
    match action {
        CcAction::VolumeDown => {
            let _ = deps.system.dispatch_hotkey(HotkeyAction::VolumeDown);
        }
        CcAction::VolumeUp => {
            let _ = deps.system.dispatch_hotkey(HotkeyAction::VolumeUp);
        }
        CcAction::VolumeMuteToggle => {
            let _ = deps.system.dispatch_hotkey(HotkeyAction::VolumeMute);
        }
        CcAction::BrightnessDown => {
            remember_brightness(
                ctx,
                deps.system.dispatch_hotkey(HotkeyAction::BrightnessDown),
            );
        }
        CcAction::BrightnessUp => {
            remember_brightness(ctx, deps.system.dispatch_hotkey(HotkeyAction::BrightnessUp));
        }
        CcAction::BluetoothToggle => {
            let _ = deps.system.dispatch_hotkey(HotkeyAction::BluetoothToggle);
        }
        CcAction::LayoutToggle => {
            // The SAME primary-toggle + set-profile seam the floating mode
            // button drives (`apply_layout_profile`); the vdock density mirror
            // self-heals next frame in `mount_dock_chrome`.
            let next = crate::layout_mode_primary_toggle(deps.system.layout_profile());
            deps.system.set_layout_profile(next, ctx);
            if next == LayoutProfile::Car {
                deps.nav.surface = Surface::AutoHome;
                deps.nav.expanded = true;
            }
            deps.construct.control_center_open = false;
        }
        CcAction::OpenSettings(section) => {
            deps.system.open_settings_section(*section);
            deps.nav.surface = Surface::System;
            deps.nav.expanded = true;
            deps.construct.control_center_open = false;
        }
        CcAction::OpenDesktop => {
            deps.nav.surface = Surface::Desktop;
            deps.nav.expanded = true;
            deps.construct.control_center_open = false;
        }
        CcAction::FocusSession(id) => {
            let _ = deps.session_rail.focus_session(id);
            deps.nav.surface = Surface::Desktop;
            deps.nav.expanded = true;
            deps.construct.control_center_open = false;
        }
        CcAction::OpenFileOperations => {
            crate::route_file_operation_progress_request(deps.files, deps.nav);
            deps.construct.control_center_open = false;
        }
    }
}

/// Remember the seat-acknowledged brightness fraction from a nudge's returned
/// [`OsdLevel`] so the row reflects the write between snapshot republishes. A
/// refused write returns `None` and the display honestly keeps the old value.
fn remember_brightness(ctx: &egui::Context, osd: Option<OsdLevel>) {
    if let Some(osd) = osd {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(BRIGHTNESS_KEY), osd.level));
    }
}

/// The last seat-acknowledged brightness fraction, if any nudge landed.
fn stored_brightness(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|d| d.get_temp(egui::Id::new(BRIGHTNESS_KEY)))
}

// ──────────────────────────── painting ────────────────────────────

/// Paint the RADIUS_XL card and its sections; returns the clicked action.
fn paint_panel(ui: &mut egui::Ui, rect: egui::Rect, model: &PanelModel) -> Option<CcAction> {
    let ctx = ui.ctx().clone();
    // Swallow in-card clicks that miss a control (never a scrim dismiss).
    let _bg = ui.interact(rect, egui::Id::new(PANEL_BG_KEY), egui::Sense::click());
    ui.painter().rect(
        rect,
        Style::RADIUS_XL,
        Style::resolve_color(&ctx, Style::SURFACE),
        egui::Stroke::new(1.0, Style::resolve_color(&ctx, Style::BORDER)),
        egui::StrokeKind::Outside,
    );

    let inner = rect.shrink(PANEL_PAD);
    let mut y = inner.top();
    let mut action: Option<CcAction> = None;

    if let Some(volume) = &model.volume {
        let row = egui::Rect::from_min_size(
            egui::pos2(inner.left(), y),
            egui::vec2(inner.width(), ROW_H),
        );
        action = action.or(volume_row(ui, row, volume));
        y = row.bottom() + SECTION_GAP;
    }
    if let Some(percent) = model.brightness {
        let row = egui::Rect::from_min_size(
            egui::pos2(inner.left(), y),
            egui::vec2(inner.width(), ROW_H),
        );
        action = action.or(brightness_row(ui, row, percent));
        y = row.bottom() + SECTION_GAP;
    }
    if !model.tiles.is_empty() {
        let (grid_action, bottom) = tile_grid(ui, inner, y, &model.tiles);
        action = action.or(grid_action);
        y = bottom + SECTION_GAP;
    }
    if !model.sessions.is_empty() {
        let (list_action, bottom) =
            session_list(ui, inner, y, &model.sessions, model.sessions_total);
        action = action.or(list_action);
        y = bottom + SECTION_GAP;
    }
    if let Some(file_ops) = &model.file_ops {
        let row = egui::Rect::from_min_size(
            egui::pos2(inner.left(), y),
            egui::vec2(inner.width(), ROW_H),
        );
        action = action.or(file_ops_row(ui, row, file_ops));
    }
    action
}

/// A small square glyph button; returns `true` on click. Falls back to a
/// status dot when the glyph cannot paint (a registry miss fails soft, §7).
fn glyph_button(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    glyph: Glyph,
    tint: egui::Color32,
    label: &str,
) -> bool {
    let resp = ui.interact(rect, id, egui::Sense::click());
    let ctx = ui.ctx();
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            Style::RADIUS_S,
            Style::resolve_color(ctx, Style::SURFACE_HI),
        );
    }
    paint_glyph(ui, rect.shrink(Style::SP_XS), glyph, tint);
    install_button_accessibility(ctx, id, rect, label);
    resp.clicked()
}

/// Paint a [`Glyph`] into `rect` (Carbon first, YAMIS texture fallback, then
/// an honest dot if neither rasterizes).
fn paint_glyph(ui: &egui::Ui, rect: egui::Rect, glyph: Glyph, tint: egui::Color32) {
    let painted = match glyph {
        Glyph::Carbon(name) => paint_carbon(ui.painter(), rect, name, tint),
        Glyph::Icon(id) => {
            let size = rect.width().min(rect.height());
            icon_texture(ui.ctx(), id, size, tint).is_some_and(|tex| {
                let draw = egui::Rect::from_center_size(rect.center(), egui::vec2(size, size));
                let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                ui.painter().image(tex.id(), draw, uv, egui::Color32::WHITE);
                true
            })
        }
    };
    if !painted {
        ui.painter()
            .circle_filled(rect.center(), Style::SP_XS, tint);
    }
}

/// The shared level-row meter (the tray flyout's meter reading, reused).
fn paint_meter(painter: &egui::Painter, rect: egui::Rect, fraction: f32, tint: egui::Color32) {
    painter.rect_filled(rect, Style::RADIUS_S, tint.gamma_multiply(0.25));
    let fill = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            fraction.clamp(0.0, 1.0).mul_add(rect.width(), rect.left()),
            rect.bottom(),
        ),
    );
    painter.rect_filled(fill, Style::RADIUS_S, tint);
}

/// A level row's shared frame: lead glyph, label, meter, − / + steppers.
/// Returns `(lead_clicked, minus_clicked, plus_clicked)`.
fn level_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    key: &'static str,
    glyph: Glyph,
    lead_interactive: bool,
    label: &str,
    fraction: f32,
) -> (bool, bool, bool) {
    let ctx = ui.ctx();
    let text = Style::resolve_color(ctx, Style::TEXT);
    let side = rect.height().min(Style::SP_XL);
    let lead = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - side * 0.5),
        egui::vec2(side, side),
    );
    let lead_clicked = if lead_interactive {
        glyph_button(ui, lead, egui::Id::new((key, "lead")), glyph, text, label)
    } else {
        paint_glyph(ui, lead.shrink(Style::SP_XS), glyph, text);
        false
    };
    // − / + steppers on the right.
    let plus = egui::Rect::from_min_size(
        egui::pos2(rect.right() - side, rect.center().y - side * 0.5),
        egui::vec2(side, side),
    );
    let minus = plus.translate(egui::vec2(-(side + Style::SP_XS), 0.0));
    let minus_clicked = glyph_button(
        ui,
        minus,
        egui::Id::new((key, "minus")),
        Glyph::Carbon("list-remove"),
        text,
        "Step down",
    );
    let plus_clicked = glyph_button(
        ui,
        plus,
        egui::Id::new((key, "plus")),
        Glyph::Carbon("list-add"),
        text,
        "Step up",
    );
    // Label over the meter, between the glyph and the steppers.
    let left = lead.right() + Style::SP_S;
    let right = minus.left() - Style::SP_S;
    ui.painter().text(
        egui::pos2(left, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(Style::TYPE_FOOTNOTE),
        text,
    );
    paint_meter(
        &ui.painter().clone(),
        egui::Rect::from_min_max(
            egui::pos2(left, rect.bottom() - Style::SP_M),
            egui::pos2(right.max(left + 1.0), rect.bottom() - Style::SP_S),
        ),
        fraction,
        Style::resolve_color(ctx, Style::ACCENT),
    );
    (lead_clicked, minus_clicked, plus_clicked)
}

/// The volume row: mute-toggle glyph + live level + ± steps (the mixer seam).
fn volume_row(ui: &egui::Ui, rect: egui::Rect, model: &VolumeModel) -> Option<CcAction> {
    let glyph = Glyph::Carbon(volume_glyph(model));
    let label = if model.muted {
        format!("Volume {}% - muted", model.volume)
    } else {
        format!("Volume {}%", model.volume)
    };
    let (lead, minus, plus) = level_row(
        ui,
        rect,
        "shell-cc-volume",
        glyph,
        true,
        &label,
        f32::from(model.volume) / 100.0,
    );
    if lead {
        Some(CcAction::VolumeMuteToggle)
    } else if minus {
        Some(CcAction::VolumeDown)
    } else if plus {
        Some(CcAction::VolumeUp)
    } else {
        None
    }
}

/// The muted/low/high volume glyph for the current reading.
const fn volume_glyph(model: &VolumeModel) -> &'static str {
    if model.muted {
        "audio-volume-muted"
    } else if model.volume < 50 {
        "audio-volume-low"
    } else {
        "audio-volume-high"
    }
}

/// The brightness row: live percent + ± steps (backlight/DDC via the seam).
fn brightness_row(ui: &egui::Ui, rect: egui::Rect, percent: u8) -> Option<CcAction> {
    let (_, minus, plus) = level_row(
        ui,
        rect,
        "shell-cc-brightness",
        Glyph::Icon(IconId::DisplaySettings),
        false,
        &format!("Brightness {percent}%"),
        f32::from(percent) / 100.0,
    );
    if minus {
        Some(CcAction::BrightnessDown)
    } else if plus {
        Some(CcAction::BrightnessUp)
    } else {
        None
    }
}

/// The 2-column tile grid. Returns the clicked action + the grid's bottom y.
fn tile_grid(
    ui: &egui::Ui,
    inner: egui::Rect,
    top: f32,
    tiles: &[Tile],
) -> (Option<CcAction>, f32) {
    let tile_w = (inner.width() - GRID_GAP) / 2.0;
    let mut action = None;
    let mut bottom = top;
    for (idx, tile) in tiles.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // tiny grid indices
        let (col, row) = ((idx % 2) as f32, (idx / 2) as f32);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                col.mul_add(tile_w + GRID_GAP, inner.left()),
                row.mul_add(TILE_H + GRID_GAP, top),
            ),
            egui::vec2(tile_w, TILE_H),
        );
        if tile_cell(ui, rect, idx, tile) {
            action = Some(tile.action.clone());
        }
        bottom = bottom.max(rect.bottom());
    }
    (action, bottom)
}

/// One RADIUS_L tile cell; returns `true` on click.
fn tile_cell(ui: &egui::Ui, rect: egui::Rect, idx: usize, tile: &Tile) -> bool {
    let ctx = ui.ctx();
    let id = egui::Id::new(("shell-cc-tile", idx));
    let resp = ui.interact(rect, id, egui::Sense::click());
    let fill = if resp.hovered() {
        Style::resolve_color(ctx, Style::SURFACE_HI)
    } else {
        Style::resolve_color(ctx, Style::SURFACE)
    };
    let stroke = if tile.active {
        egui::Stroke::new(2.0, Style::resolve_color(ctx, Style::ACCENT))
    } else {
        egui::Stroke::new(1.0, Style::resolve_color(ctx, Style::BORDER))
    };
    ui.painter().rect(
        rect,
        Style::RADIUS_L,
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let text = Style::resolve_color(ctx, Style::TEXT);
    let glyph_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + Style::SP_S + Style::SP_M * 0.5,
            rect.center().y,
        ),
        egui::vec2(Style::SP_L, Style::SP_L),
    );
    paint_glyph(ui, glyph_rect, tile.glyph, text);
    let text_x = glyph_rect.right() + Style::SP_S;
    ui.painter().text(
        egui::pos2(text_x, rect.top() + Style::SP_S),
        egui::Align2::LEFT_TOP,
        tile.title,
        egui::FontId::proportional(Style::TYPE_FOOTNOTE),
        text,
    );
    ui.painter().text(
        egui::pos2(
            text_x,
            rect.top() + Style::SP_S + Style::TYPE_FOOTNOTE + Style::SP_XS,
        ),
        egui::Align2::LEFT_TOP,
        &tile.caption,
        egui::FontId::proportional(Style::TYPE_FOOTNOTE),
        Style::resolve_color(ctx, Style::TEXT_DIM),
    );
    if let Some(tone) = tile.tone {
        ui.painter().circle_filled(
            egui::pos2(rect.right() - Style::SP_S, rect.top() + Style::SP_S),
            Style::SP_XS * 0.75,
            Style::resolve_color(ctx, tone),
        );
    }
    install_button_accessibility(ctx, id, rect, tile.title);
    resp.clicked()
}

/// The VDI session header + rows. Returns the clicked action + the bottom y.
fn session_list(
    ui: &egui::Ui,
    inner: egui::Rect,
    top: f32,
    sessions: &[SessionRow],
    total: usize,
) -> (Option<CcAction>, f32) {
    let ctx = ui.ctx();
    ui.painter().text(
        egui::pos2(inner.left(), top),
        egui::Align2::LEFT_TOP,
        if total > sessions.len() {
            format!("Sessions ({total})")
        } else {
            "Sessions".to_owned()
        },
        egui::FontId::proportional(Style::TYPE_FOOTNOTE),
        Style::resolve_color(ctx, Style::TEXT_DIM),
    );
    let mut y = top + HEADER_H;
    let mut action = None;
    for (idx, session) in sessions.iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(inner.left(), y),
            egui::vec2(inner.width(), SESSION_ROW_H),
        );
        let id = egui::Id::new(("shell-cc-session", idx));
        let resp = ui.interact(rect, id, egui::Sense::click());
        let fill = if resp.hovered() {
            Style::resolve_color(ctx, Style::SURFACE_HI)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, Style::RADIUS_M, fill);
        let text = Style::resolve_color(ctx, Style::TEXT);
        let glyph_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + Style::SP_S + Style::SP_XS, rect.center().y),
            egui::vec2(Style::SP_M, Style::SP_M),
        );
        paint_glyph(ui, glyph_rect, Glyph::Carbon("view"), text);
        ui.painter().text(
            egui::pos2(glyph_rect.right() + Style::SP_S, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &session.label,
            egui::FontId::proportional(Style::TYPE_FOOTNOTE),
            text,
        );
        ui.painter().text(
            egui::pos2(rect.right() - Style::SP_S, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            session.badge,
            egui::FontId::proportional(Style::TYPE_FOOTNOTE),
            Style::resolve_color(ctx, Style::TEXT_DIM),
        );
        install_button_accessibility(ctx, id, rect, &session.label);
        if resp.clicked() {
            // A row without a broker id (the pending-request fallback) still
            // routes Desktop honestly — there is nothing to focus yet.
            action = Some(
                session
                    .id
                    .clone()
                    .map_or(CcAction::OpenDesktop, |sid| CcAction::FocusSession(sid)),
            );
        }
        y = rect.bottom() + GRID_GAP;
    }
    (action, y - GRID_GAP)
}

/// The file-operation progress row (present only while jobs are active).
fn file_ops_row(ui: &egui::Ui, rect: egui::Rect, model: &FileOpModel) -> Option<CcAction> {
    let ctx = ui.ctx();
    let id = egui::Id::new("shell-cc-file-ops");
    let resp = ui.interact(rect, id, egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            Style::RADIUS_M,
            Style::resolve_color(ctx, Style::SURFACE_HI),
        );
    }
    let text = Style::resolve_color(ctx, Style::TEXT);
    let side = rect.height().min(Style::SP_XL);
    let lead = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - side * 0.5),
        egui::vec2(side, side),
    );
    paint_glyph(
        ui,
        lead.shrink(Style::SP_XS),
        Glyph::Carbon("download"),
        text,
    );
    let left = lead.right() + Style::SP_S;
    let label = if model.active == 1 {
        model.label.clone()
    } else {
        format!("{} - {} operations", model.label, model.active)
    };
    ui.painter().text(
        egui::pos2(left, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(Style::TYPE_FOOTNOTE),
        text,
    );
    let meter = egui::Rect::from_min_max(
        egui::pos2(left, rect.bottom() - Style::SP_M),
        egui::pos2(rect.right() - Style::SP_S, rect.bottom() - Style::SP_S),
    );
    // A known average paints the fill; all-starting paints the honest empty
    // track with a "starting" note — never a fabricated percent.
    model.fraction.map_or_else(
        || {
            paint_meter(
                &ui.painter().clone(),
                meter,
                0.0,
                Style::resolve_color(ctx, Style::ACCENT),
            );
            ui.painter().text(
                egui::pos2(rect.right() - Style::SP_S, rect.top() + Style::SP_XS),
                egui::Align2::RIGHT_TOP,
                "starting",
                egui::FontId::proportional(Style::TYPE_FOOTNOTE),
                Style::resolve_color(ctx, Style::TEXT_DIM),
            );
        },
        |fraction| {
            paint_meter(
                &ui.painter().clone(),
                meter,
                fraction,
                Style::resolve_color(ctx, Style::ACCENT),
            );
        },
    );
    install_button_accessibility(ctx, id, rect, "File operations");
    resp.clicked().then_some(CcAction::OpenFileOperations)
}

/// Install a minimal AccessKit button node for a painted control (the
/// `install_layout_mode_button_accessibility` idiom).
fn install_button_accessibility(ctx: &egui::Context, id: egui::Id, rect: egui::Rect, label: &str) {
    let label = label.to_owned();
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.as_str());
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
        node.add_action(egui::accesskit::Action::Click);
    });
}

// ──────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use mde_seat::{MixerStatus, MixerStrip, Seat, StripOrigin};
    use std::path::PathBuf;

    /// One test-owned bundle of every state the mount seam forwards.
    struct Harness {
        construct: ConstructChrome,
        system: SystemState,
        nav: Nav,
        session_rail: SessionRailState,
        files: FileBrowser,
        web: WebState,
        mesh: MeshSummary,
        segments: StatusSegments,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                construct: ConstructChrome::default(),
                system: SystemState::default(),
                nav: Nav::default(),
                session_rail: SessionRailState::default(),
                files: mde_files_egui::real_browser(),
                web: WebState::default(),
                mesh: MeshSummary::default(),
                segments: StatusSegments::default(),
            }
        }

        fn deps(&mut self) -> ControlCenterDeps<'_> {
            ControlCenterDeps {
                construct: &mut self.construct,
                system: &mut self.system,
                nav: &mut self.nav,
                session_rail: &mut self.session_rail,
                files: &mut self.files,
                web: &mut self.web,
                mesh: &self.mesh,
                segments: &self.segments,
                local_host: "eagle",
            }
        }
    }

    /// Run one headless frame mounting the Control Center, tessellating on the
    /// CPU (the DRM runner's path minus the GPU — the system/tests idiom).
    fn frame(ctx: &egui::Context, harness: &mut Harness, events: Vec<egui::Event>) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| mount(ctx, harness.deps()));
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    fn master(volume: u8, muted: bool) -> MixerStatus {
        MixerStatus {
            master: MixerStrip {
                id: "master".to_owned(),
                name: "Master".to_owned(),
                origin: StripOrigin::HostSession,
                volume,
                muted,
            },
            strips: Vec::new(),
        }
    }

    // ── the overlay lifecycle (open flag · backdrop · dismissal) ────────────

    #[test]
    fn a_closed_control_center_paints_nothing() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut harness = Harness::new();
        assert_eq!(
            frame(&ctx, &mut harness, Vec::new()),
            0,
            "a closed Control Center must add zero primitives"
        );
    }

    #[test]
    fn the_open_flag_renders_the_panel_and_the_backdrop() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut harness = Harness::new();
        harness.construct.control_center_open = true;
        // Settle the reveal spring across a few frames, then inspect the last.
        for _ in 0..10 {
            let _ = frame(&ctx, &mut harness, Vec::new());
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| mount(ctx, harness.deps()));
        // The SCRIM_THIN backdrop covers the whole screen…
        let scrim = out.shapes.iter().any(|s| match &s.shape {
            egui::Shape::Rect(r) => r.rect.width() >= 1280.0 && r.rect.height() >= 800.0,
            _ => false,
        });
        assert!(scrim, "no full-screen backdrop rect was painted");
        // …and the card produced real geometry.
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the open Control Center tessellated nothing"
        );
    }

    #[test]
    fn escape_clears_the_open_flag() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut harness = Harness::new();
        harness.construct.control_center_open = true;
        let _ = frame(
            &ctx,
            &mut harness,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(
            !harness.construct.control_center_open,
            "Escape must dismiss the Control Center"
        );
    }

    #[test]
    fn a_scrim_click_outside_the_card_clears_the_open_flag() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut harness = Harness::new();
        harness.construct.control_center_open = true;
        // Let the reveal settle so the card sits at rest before clicking.
        for _ in 0..10 {
            let _ = frame(&ctx, &mut harness, Vec::new());
        }
        // Far left — well outside the top-right card.
        let pos = egui::pos2(40.0, 400.0);
        let _ = frame(
            &ctx,
            &mut harness,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let _ = frame(
            &ctx,
            &mut harness,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(
            !harness.construct.control_center_open,
            "a click on the scrim must dismiss the Control Center"
        );
    }

    #[test]
    fn the_control_center_intent_toggles_the_flag_through_the_mount() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut harness = Harness::new();
        harness.construct.dispatch(&crate::construct::ChromeInput {
            super_tap: false,
            super_tab: false,
            app_expanded: false,
            remote_session_focused: false,
            edges: vec![crate::construct::EdgeSwipe {
                edge: mde_egui::Edge::Top,
                x_frac: Some(0.9),
            }],
            now: std::time::Duration::ZERO,
        });
        let _ = frame(&ctx, &mut harness, Vec::new());
        assert!(
            harness.construct.control_center_open,
            "the queued ControlCenter intent must open the overlay"
        );
    }

    // ── the volume tile ↔ the real mixer seam ───────────────────────────────

    #[test]
    fn the_volume_row_reflects_the_seat_mixer_probe() {
        let mut system = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.mixer = Probe::Present(master(40, false));
        system.set_snapshot_for_test(snap);
        assert_eq!(
            volume_model(system.snapshot()),
            Some(VolumeModel {
                volume: 40,
                muted: false
            }),
            "the volume row must mirror the mixer probe exactly"
        );
        // An Absent mixer is an honest None — the grid then deep-links Audio.
        assert_eq!(volume_model(None), None);
    }

    #[test]
    fn a_volume_step_drives_the_seat_and_reports_honestly() {
        // The EXACT seam the tile's + button calls. On a host with PipeWire the
        // write lands and the cached strip accumulates; on the headless farm
        // the write is refused. Assert the honest coupling either way (§7 — a
        // failed write must never move the displayed level).
        let mut system = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.mixer = Probe::Present(master(40, false));
        system.set_snapshot_for_test(snap);
        let osd = system.dispatch_hotkey(HotkeyAction::VolumeUp);
        let shown = volume_model(system.snapshot()).map(|m| m.volume);
        assert_eq!(
            osd.is_some(),
            shown == Some(45),
            "an acknowledged write must move the level by exactly one step"
        );
        assert_eq!(
            osd.is_none(),
            shown == Some(40),
            "a refused write must leave the displayed level untouched"
        );
    }

    #[test]
    fn the_volume_action_routes_through_apply_to_the_same_seam() {
        let ctx = egui::Context::default();
        let mut harness = Harness::new();
        let mut snap = Seat::new().snapshot();
        snap.mixer = Probe::Present(master(40, false));
        harness.system.set_snapshot_for_test(snap);
        apply(&CcAction::VolumeUp, &ctx, &mut harness.deps());
        let shown = volume_model(harness.system.snapshot()).map(|m| m.volume);
        assert!(
            shown == Some(45) || shown == Some(40),
            "apply must reach the mixer seam: stepped or honestly refused, got {shown:?}"
        );
    }

    // ── deep-links, the mode toggle, and dismissal-on-route ─────────────────

    #[test]
    fn a_settings_deep_link_routes_lands_on_the_section_and_dismisses() {
        let ctx = egui::Context::default();
        let mut harness = Harness::new();
        harness.construct.control_center_open = true;
        apply(
            &CcAction::OpenSettings(SettingsSection::Network),
            &ctx,
            &mut harness.deps(),
        );
        assert_eq!(harness.nav.surface, Surface::System);
        assert!(harness.nav.expanded);
        assert_eq!(
            harness.system.settings_section_for_test(),
            SettingsSection::Network,
            "the deep-link must rest the Settings rail on its target section"
        );
        assert!(
            !harness.construct.control_center_open,
            "a route must dismiss the overlay"
        );
    }

    #[test]
    fn the_layout_toggle_flips_the_profile_through_the_shared_seam() {
        let ctx = egui::Context::default();
        let mut harness = Harness::new();
        harness.construct.control_center_open = true;
        let before = harness.system.layout_profile();
        apply(&CcAction::LayoutToggle, &ctx, &mut harness.deps());
        let after = harness.system.layout_profile();
        assert_eq!(
            after,
            crate::layout_mode_primary_toggle(before),
            "the tile must drive the SAME primary toggle the mode button uses"
        );
        if after == LayoutProfile::Car {
            assert_eq!(
                harness.nav.surface,
                Surface::AutoHome,
                "entering Car lands on the Auto-Mode home (apply_layout_profile parity)"
            );
        }
        // Flip back so the persisted profile never leaks between tests.
        apply(&CcAction::LayoutToggle, &ctx, &mut harness.deps());
        assert_eq!(harness.system.layout_profile(), before);
    }

    // ── composition: every shipped tile is state-backed or a real route ─────

    #[test]
    fn an_all_absent_seat_degrades_every_tile_to_an_honest_route() {
        // Default states (no mixer, no backlight, no adapter, no sessions, no
        // ops): nothing may fabricate a control — the §7 zero-dead-tiles rule.
        let mesh = MeshSummary::default();
        let segments = StatusSegments::default();
        let model = fold_model(
            None,
            None,
            LayoutProfile::Construct,
            &mesh,
            &segments,
            &[],
            None,
            None,
        );
        assert_eq!(model.volume, None, "no mixer → no volume slider");
        assert_eq!(model.brightness, None, "no device → no brightness slider");
        assert!(model.sessions.is_empty());
        assert_eq!(model.file_ops, None, "no active ops → no progress row");
        let actions: Vec<&CcAction> = model.tiles.iter().map(|t| &t.action).collect();
        assert!(
            actions.contains(&&CcAction::OpenSettings(SettingsSection::Network)),
            "the network tile deep-links Settings → Network"
        );
        assert!(
            actions.contains(&&CcAction::OpenSettings(SettingsSection::Bluetooth)),
            "no adapter → the BT tile deep-links Settings → Bluetooth"
        );
        assert!(
            actions.contains(&&CcAction::OpenSettings(SettingsSection::Displays)),
            "no brightness seam → the Displays deep-link tile"
        );
        assert!(
            actions.contains(&&CcAction::OpenSettings(SettingsSection::Audio)),
            "no mixer → the Audio deep-link tile"
        );
        assert!(
            actions.contains(&&CcAction::LayoutToggle),
            "the mode toggle is always a real control"
        );
        assert!(
            actions.contains(&&CcAction::OpenDesktop),
            "no sessions → the Remote-Sessions deep-link tile"
        );
        // The honest pre-read mesh caption — never invented counts.
        let network = model
            .tiles
            .iter()
            .find(|t| t.title == "Network")
            .expect("network tile");
        assert_eq!(network.caption, "Mesh not read yet");
        // The height math covers exactly the painted sections.
        assert!(model.height() > PANEL_PAD * 2.0);
    }

    #[test]
    fn present_probes_swap_deep_links_for_live_controls() {
        let mut system = SystemState::default();
        let mut snap = Seat::new().snapshot();
        snap.mixer = Probe::Present(master(70, true));
        system.set_snapshot_for_test(snap);
        let mesh = MeshSummary::default();
        let segments = StatusSegments::default();
        let model = fold_model(
            system.snapshot(),
            None,
            LayoutProfile::Construct,
            &mesh,
            &segments,
            &[],
            None,
            None,
        );
        assert_eq!(
            model.volume,
            Some(VolumeModel {
                volume: 70,
                muted: true
            })
        );
        assert!(
            !model
                .tiles
                .iter()
                .any(|t| t.action == CcAction::OpenSettings(SettingsSection::Audio)),
            "a Present mixer must retire the Audio deep-link tile"
        );
        assert_eq!(
            volume_glyph(&VolumeModel {
                volume: 70,
                muted: true
            }),
            "audio-volume-muted"
        );
        assert_eq!(
            volume_glyph(&VolumeModel {
                volume: 70,
                muted: false
            }),
            "audio-volume-high"
        );
        assert_eq!(
            volume_glyph(&VolumeModel {
                volume: 20,
                muted: false
            }),
            "audio-volume-low"
        );
    }

    // ── the file-operation row ──────────────────────────────────────────────

    #[test]
    fn the_file_operation_row_exists_only_while_ops_are_active() {
        assert_eq!(file_ops_model(None, None), None);
        let idle = OperationProgressSummary {
            active: 0,
            known_progress: 0,
            fraction: None,
            label: "done.iso".to_owned(),
        };
        assert_eq!(
            file_ops_model(Some(idle), None),
            None,
            "an idle summary paints no row"
        );
        let files = OperationProgressSummary {
            active: 2,
            known_progress: 1,
            fraction: Some(0.5),
            label: "big.iso".to_owned(),
        };
        let browser = OperationProgressSummary {
            active: 1,
            known_progress: 1,
            fraction: Some(0.9),
            label: "page.pdf".to_owned(),
        };
        let row = file_ops_model(Some(files), Some(browser)).expect("active row");
        assert_eq!(
            (row.active, row.fraction, row.label.as_str()),
            (2, Some(0.5), "big.iso"),
            "Files summaries take precedence over Browser downloads \
             (shell_file_operation_progress parity)"
        );
    }

    // ── the VDI session list ────────────────────────────────────────────────

    fn temp_bus(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("mde-cc-{tag}-{n}"));
        std::fs::create_dir_all(&root).expect("mkroot");
        root
    }

    #[test]
    fn session_rows_project_the_rail_and_focus_routes_desktop() {
        let root = temp_bus("focus");
        Persist::open(root.clone())
            .expect("open bus")
            .write(
                "action/vdi/session",
                Priority::Default,
                None,
                Some(
                    r#"{"op":"open","id":"s1","serving_peer":"oak","vm_id":"win11","client_peer":"eagle"}"#,
                ),
            )
            .expect("write session action");
        let ctx = egui::Context::default();
        let mut harness = Harness::new();
        harness.session_rail = SessionRailState::with_bus_root(root.clone());
        harness.construct.control_center_open = true;

        let entries = harness.session_rail.entries("eagle");
        let rows = session_rows(&entries);
        assert_eq!(
            rows,
            vec![SessionRow {
                id: Some("s1".to_owned()),
                label: "oak win11".to_owned(),
                badge: "VDI",
            }],
            "the list is the taskbar rail's exact projection"
        );

        apply(
            &CcAction::FocusSession("s1".to_owned()),
            &ctx,
            &mut harness.deps(),
        );
        assert_eq!(harness.nav.surface, Surface::Desktop);
        assert!(harness.nav.expanded);
        assert!(!harness.construct.control_center_open);
        let refreshed = harness.session_rail.entries("eagle");
        assert_eq!(
            refreshed[0].protocol(),
            "LIVE",
            "the focus action drives the SAME focus_session seam the taskbar uses"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
