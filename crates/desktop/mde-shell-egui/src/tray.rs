//! The dock's **status quads** (VDOCK-3) — the two stacked 2×2 status grids in
//! the vertical dock's bottom band: quad 1 Chat[badge] · Bluetooth · Volume ·
//! Battery over quad 2 Status · Signal · Peers · Sessions. Every cell renders
//! **real state** (§7) folded from the same sources the retired horizontal tray
//! read — the world-readable mesh-status snapshot ([`MeshSummary`], the Status /
//! Signal / Peers dots), the ONE `mde-seat` [`SeatSnapshot`] (Bluetooth / Volume /
//! Battery), and the Chat unread tally — and **routes** to its owning surface on a
//! click (no flyouts, lock #15).
//!
//! State rides a **tiny corner dot** in the OK/WARN/DANGER tokens while the glyph
//! keeps one tint (lock W9); the Chat cell carries an unread count **badge** in the
//! dot's place. The folds (glyph pick, dot tone, battery fill ladder, Chat badge,
//! click routing) are pure — no egui `Context`, no IO — so they're unit-tested
//! directly; the only egui here is the quad layout ([`status_quads`]) + the shared
//! cell painter ([`paint_icon_view`]).
//!
//! This is what survived VDOCK-6b's teardown of the old horizontal taskbar tray
//! (the `^` overflow chevron, the Volume/Bluetooth/Chat micro-flyouts, and their
//! `wpctl`/`BlueZ` verb seam are gone — the System surface owns those host-control
//! writes now); the pure status folds + the 2×2 quad render are all that the
//! vertical dock needs.

use mde_egui::egui::{self, FontId};
use mde_egui::Style;

use mde_cosmic_applet::LighthouseHealth;
use mde_seat::{Battery, BatteryState, Probe, SeatSnapshot};
use mde_theme::brand::icons::IconId;

use crate::chrome::MeshSummary;
use crate::dock::{icon_texture, Surface};

/// The corner status dot's radius (lock W9) — tiny, token-derived (§4).
const DOT_R: f32 = Style::SP_XS / 2.0;

/// Charge (%) below which a **draining** system pack's dot reads amber "low",
/// and at or below which it reads red "critical" (lock W8). A charging or full
/// pack is never amber/red (it's improving); these bite only while the pack is
/// actually discharging. Moved verbatim from the retired chrome strip.
const BATTERY_LOW: f64 = 20.0;
const BATTERY_CRITICAL: f64 = 5.0;

// ─────────────────────────────── the tray model ──────────────────────────────

/// One status slot. The [`STATUS_QUADS`] layout and the click routing ([`route`])
/// are keyed off this — pure and unit-tested. (`pub`, not `pub(crate)`, is the
/// `clippy::redundant_pub_crate` form for crate-visible items in this private
/// module, like `dock::TASKBAR_H`.) `Hash` so [`quad_cell_id`] can key a stable
/// per-cell `egui::Id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayItem {
    /// The live VDI session marker.
    Sessions,
    /// The unified Chat surface's unread tally (count badge, lock W7).
    Chat,
    /// Bluetooth adapter power (from the seat snapshot).
    Bluetooth,
    /// Master output volume / mute (from the seat snapshot).
    Volume,
    /// The system battery pack — charge-fill glyph + bolt + tone dot (lock W8).
    Battery,
    /// Worst-of mesh lighthouse health (the fleet Status verdict).
    Status,
    /// Mesh reachability.
    Signal,
    /// Peer directory presence.
    Peers,
}

/// Where a status cell's routing lands (lock W7/#15): the surface that owns its
/// state — seat hardware to System, mesh telemetry to the Mesh Map, Chat to Chat,
/// the live session to Desktop.
const fn route(item: TrayItem) -> Surface {
    match item {
        TrayItem::Sessions => Surface::Desktop,
        TrayItem::Chat => Surface::Chat,
        TrayItem::Bluetooth | TrayItem::Volume | TrayItem::Battery => Surface::System,
        TrayItem::Status | TrayItem::Signal | TrayItem::Peers => Surface::MeshView,
    }
}

// ─────────────────────────────── the pure folds ──────────────────────────────

/// Everything the status quads fold from, bundled so the dock hands one immutable
/// view of the frame's live state.
pub struct TrayInputs<'a> {
    /// The mesh-status snapshot fold — the same [`crate::chrome::ChromeState`]
    /// poll product the retired strip read (one poll, no second reader).
    pub mesh: &'a MeshSummary,
    /// The ONE `mde-seat` snapshot (Bluetooth / Volume / Battery), `None`
    /// before the System state's first poll lands.
    pub seat: Option<&'a SeatSnapshot>,
    /// The whole-mesh Chat unread tally (folded alerts + clips + messages).
    pub unread: usize,
    /// `true` while a VDI session is connected/pending — the Sessions cell's
    /// honest tone.
    pub session_active: bool,
}

/// One resolved status icon: the glyph to draw, its corner-dot tone, whether the
/// charging bolt overlays it, and the Chat count badge (which replaces the
/// dot). Pure data — the fold is unit-tested without egui.
struct IconView {
    /// The brand glyph (16px tray set, or the reused Node/MeshView/Chat).
    glyph: IconId,
    /// The corner status dot's tone — an OK/WARN/DANGER/dim `Style` token.
    dot: egui::Color32,
    /// Overlay [`IconId::BatteryBolt`] in the same rect (charging, lock W8).
    bolt: bool,
    /// The unread count badge ("99+"-capped); drawn instead of the dot.
    badge: Option<String>,
}

/// Fold one status item's live view from the frame's inputs — the single
/// glyph/dot/bolt/badge authority the quad cells render through.
fn icon_view(item: TrayItem, inputs: &TrayInputs<'_>) -> IconView {
    let plain = |glyph: IconId, dot: egui::Color32| IconView {
        glyph,
        dot,
        bolt: false,
        badge: None,
    };
    match item {
        // In VDOCK-3's quad the Sessions cell is always present, so a quiet dot
        // reads dim with no session connected (§7 — never a fake "live").
        TrayItem::Sessions => plain(
            IconId::Sessions,
            if inputs.session_active {
                Style::OK
            } else {
                Style::TEXT_DIM
            },
        ),
        TrayItem::Chat => IconView {
            glyph: IconId::Chat,
            dot: chat_dot(inputs.unread),
            bolt: false,
            badge: chat_badge(inputs.unread),
        },
        TrayItem::Bluetooth => plain(IconId::BluetoothSmall, bluetooth_dot(inputs.seat)),
        TrayItem::Volume => {
            let (glyph, dot) = volume_view(inputs.seat);
            plain(glyph, dot)
        }
        TrayItem::Battery => {
            let (glyph, bolt, dot) = battery_indicator(inputs.seat);
            IconView {
                glyph,
                dot,
                bolt,
                badge: None,
            }
        }
        TrayItem::Status => plain(IconId::Node, status_dot(inputs.mesh)),
        TrayItem::Signal => plain(IconId::Signal, signal_dot(inputs.mesh)),
        TrayItem::Peers => plain(IconId::MeshView, peers_dot(inputs.mesh)),
    }
}

/// The Chat dot when no badge shows: accent while something waits, dim quiet.
const fn chat_dot(unread: usize) -> egui::Color32 {
    if unread > 0 {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    }
}

/// The Chat unread count badge — `None` when quiet, capped at "99+" so a
/// firehose can't stretch the cell (the same cap the retired strip used).
fn chat_badge(unread: usize) -> Option<String> {
    match unread {
        0 => None,
        1..=99 => Some(unread.to_string()),
        _ => Some("99+".to_string()),
    }
}

/// The Bluetooth dot: OK while any adapter is powered; dim when powered off,
/// absent (no `BlueZ` / no bus — the build-host case), or not yet polled.
/// Never a fabricated radio state (§7).
fn bluetooth_dot(seat: Option<&SeatSnapshot>) -> egui::Color32 {
    match seat.map(|s| &s.bluetooth) {
        Some(Probe::Present(bt)) if bt.any_adapter_powered() => Style::OK,
        _ => Style::TEXT_DIM,
    }
}

/// The Volume glyph + dot: the muted variant with a WARN dot while the master
/// output is muted (the at-a-glance "you're silent" state), the plain speaker
/// with an OK dot while live, and dim when the mixer is absent / not yet
/// polled. Never a fake level (§7).
fn volume_view(seat: Option<&SeatSnapshot>) -> (IconId, egui::Color32) {
    match seat.map(|s| &s.mixer) {
        Some(Probe::Present(m)) if m.master.muted => (IconId::VolumeMuted, Style::WARN),
        Some(Probe::Present(_)) => (IconId::Volume, Style::OK),
        _ => (IconId::Volume, Style::TEXT_DIM),
    }
}

/// The Battery slot: `(fill glyph, charging bolt, dot tone)` for the system
/// pack (lock W8). An absent backend / empty snapshot / pre-poll state reads
/// the dim empty outline — honest, never a fabricated level (§7).
fn battery_indicator(seat: Option<&SeatSnapshot>) -> (IconId, bool, egui::Color32) {
    match seat.map(|s| &s.batteries) {
        Some(Probe::Present(cells)) => {
            system_pack(cells).map_or((IconId::BatteryEmpty, false, Style::TEXT_DIM), |b| {
                (
                    battery_fill_icon(b.percentage),
                    charging(b.state),
                    battery_tone(b),
                )
            })
        }
        _ => (IconId::BatteryEmpty, false, Style::TEXT_DIM),
    }
}

/// Map a charge percentage onto the five-step Win10 fill ladder (lock W8):
/// each glyph owns the band centred on its step, so the icon reads the nearest
/// quarter — `Empty` < 12.5 ≤ `Quarter` < 37.5 ≤ `Half` < 62.5 ≤
/// `ThreeQuarter` < 87.5 ≤ `Full`.
fn battery_fill_icon(percentage: f64) -> IconId {
    if percentage < 12.5 {
        IconId::BatteryEmpty
    } else if percentage < 37.5 {
        IconId::BatteryQuarter
    } else if percentage < 62.5 {
        IconId::BatteryHalf
    } else if percentage < 87.5 {
        IconId::BatteryThreeQuarter
    } else {
        IconId::BatteryFull
    }
}

/// Whether the pack is taking charge — the bolt-overlay signal (lock W8). A
/// pending-charge pack is on AC too, so it carries the bolt like the retired
/// strip's `⚡` suffix did.
const fn charging(state: BatteryState) -> bool {
    matches!(state, BatteryState::Charging | BatteryState::PendingCharge)
}

/// The battery dot's tone for the chosen system pack — moved verbatim from the
/// retired chrome strip's `battery_tone` (the value-colour half is gone with
/// the text). A charging or full pack reads OK; a draining pack reads red at or
/// under ~5% (or when `UPower` reports it empty) and amber under ~20%; anything
/// else (a healthily draining pack, a pending state) reads the neutral dim dot.
fn battery_tone(b: &Battery) -> egui::Color32 {
    match b.state {
        BatteryState::Charging | BatteryState::FullyCharged => Style::OK,
        BatteryState::Empty => Style::DANGER,
        BatteryState::Discharging | BatteryState::PendingDischarge => {
            if b.percentage <= BATTERY_CRITICAL {
                Style::DANGER
            } else if b.percentage < BATTERY_LOW {
                Style::WARN
            } else {
                Style::TEXT_DIM
            }
        }
        BatteryState::PendingCharge | BatteryState::Unknown => Style::TEXT_DIM,
    }
}

/// Pick the system pack to summarise from a multi-battery snapshot — moved
/// verbatim from the retired chrome strip: the `PowerSupply` pack that actually
/// powers the host, else — when none is flagged (an all-peripheral snapshot) —
/// the fullest cell, so the slot never invents a reading. `None` only for an
/// empty list (the caller renders the dim empty outline).
fn system_pack(cells: &[Battery]) -> Option<&Battery> {
    cells.iter().find(|b| b.power_supply).or_else(|| {
        cells.iter().max_by(|a, b| {
            a.percentage
                .partial_cmp(&b.percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    })
}

/// The Status dot: the worst-of lighthouse verdict — OK all-healthy, DANGER
/// degraded, dim with no lighthouses in view or before the first snapshot.
const fn status_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen {
        return Style::TEXT_DIM;
    }
    match s.health {
        LighthouseHealth::AllHealthy => Style::OK,
        LighthouseHealth::Degraded => Style::DANGER,
        LighthouseHealth::None => Style::TEXT_DIM,
    }
}

/// The Signal dot: mesh reachability — OK while any peer answers, WARN when
/// the directory is populated but nobody does (isolated), dim empty/unseen.
const fn signal_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen || s.peers_total == 0 {
        Style::TEXT_DIM
    } else if s.peers_online == 0 {
        Style::WARN
    } else {
        Style::OK
    }
}

/// The Peers dot: OK all-online, WARN some-away, dim empty/unseen.
const fn peers_dot(s: &MeshSummary) -> egui::Color32 {
    if !s.seen || s.peers_total == 0 {
        Style::TEXT_DIM
    } else if s.peers_online == s.peers_total {
        Style::OK
    } else {
        Style::WARN
    }
}

/// Paint a resolved [`IconView`] into `icon_rect` at the `icon_px` glyph edge: the
/// glyph at the dim tray tint, the charging bolt overlaid in the same rect (lock
/// W8), and the unread **badge** OR — in its place — the tiny corner status **dot**
/// (lock W9). The single glyph/bolt/dot/badge painter (§6) VDOCK-3's [`quad_cell`]
/// renders each cell through.
fn paint_icon_view(
    ui: &egui::Ui,
    painter: &egui::Painter,
    view: &IconView,
    icon_rect: egui::Rect,
    icon_px: f32,
) {
    if let Some(tex) = icon_texture(ui.ctx(), view.glyph, icon_px, Style::TEXT_DIM) {
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    // The charging bolt overlays the fill glyph in the same rect (lock W8),
    // brighter than the outline so it reads at the small glyph edge.
    if view.bolt {
        if let Some(tex) = icon_texture(ui.ctx(), IconId::BatteryBolt, icon_px, Style::TEXT) {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
                .paint_at(ui, icon_rect);
        }
    }
    if let Some(text) = &view.badge {
        // The Chat unread badge — an accent pill on the glyph's top-right
        // corner, replacing the dot (the count IS the state).
        let font = FontId::proportional(Style::SMALL * 0.75);
        let galley = ui.fonts(|f| f.layout_no_wrap(text.clone(), font, Style::BG));
        let text_size = galley.size();
        let badge_rect = egui::Rect::from_center_size(
            icon_rect.right_top(),
            egui::vec2(text_size.x + Style::SP_XS, text_size.y),
        );
        painter.rect_filled(badge_rect, Style::RADIUS, Style::ACCENT);
        painter.galley(badge_rect.center() - text_size / 2.0, galley, Style::BG);
    } else {
        // The tiny corner status dot (lock W9) on the glyph's bottom-right.
        painter.circle_filled(icon_rect.right_bottom(), DOT_R, view.dot);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VDOCK-3 — the status **2×2 quads** (design `docs/design/vertical-dock.md`,
// locks #6/#8/#15/#19). Two stacked 2×2 quads anchored at the dock's bottom band:
// quad 1 = Chat[badge] · Bluetooth · Volume · Battery, quad 2 = Status · Signal ·
// Peers · Sessions. Every cell **routes** to its owning surface on a click (no
// flyouts — lock #15), reusing the SAME pure folds (`icon_view`, `route`, the
// battery ladder, the mesh dots, the Chat badge) — a re-layout, not a second
// status model (§6). Rendered by the dock's `paint_dock_frame`.
// ═══════════════════════════════════════════════════════════════════════════

/// The status-quad glyph edge (~18px, design #12/#23 — smaller than the 24px app
/// glyph) — token-derived on the 8px grid: `SP_M` (16) plus half an `SP_XS` (2).
const QUAD_ICON: f32 = Style::SP_M + Style::SP_XS / 2.0;

/// The two stacked 2×2 status quads (design #6), each row-major (top-left,
/// top-right, bottom-left, bottom-right): the seat/Chat cluster over the mesh/
/// session cluster. The one authority the render + the routing + the tests read.
pub const STATUS_QUADS: [[TrayItem; 4]; 2] = [
    [
        TrayItem::Chat,
        TrayItem::Bluetooth,
        TrayItem::Volume,
        TrayItem::Battery,
    ],
    [
        TrayItem::Status,
        TrayItem::Signal,
        TrayItem::Peers,
        TrayItem::Sessions,
    ],
];

/// The stable per-item id of a quad cell, so the render + routing are unchanged
/// but the layout is addressable — tests read a cell's settled `Rect` back to
/// click its centre (the `dock::pick_cell_id` idiom, kept distinct so a quad cell
/// never shares an id with a picker cell). Used by `quad_cell` (production) + the
/// tray + dock tests, so it's crate-visible (`pub` in this private module).
pub fn quad_cell_id(item: TrayItem) -> egui::Id {
    egui::Id::new(("vdock-status-quad-cell", item))
}

/// Render VDOCK-3's two stacked **2×2 status quads** into the dock's bottom band
/// (design #6/#8): `origin` is the top-left of the first quad and each quad is a
/// `quad`-wide × `quad`-tall 2×2 grid of `quad / 2` cells (so the two quads occupy
/// `2 · quad` of the band, VDOCK-4's system quad the remainder). Every cell folds
/// its live [`IconView`] and **routes** to its owning [`Surface`] on a click (lock
/// #15 — no flyouts); the Chat cell carries the CHAT-FIX-2 unread badge (#19).
/// Each quad is drawn as a **fully-enclosed box** — a complete 1px `Style::BORDER`
/// outline around all four sides (operator directive: enclose each colored box on
/// the nav with a full outside outline, §4 tokens). Paints through `ui.interact`
/// over explicit rects (the dock's `&Ui` idiom), so it composes inside
/// `paint_dock_frame`. Returns `true` if a cell routed.
#[allow(
    clippy::cast_precision_loss, // the quad / row / col indices are tiny (0..4)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
pub fn status_quads(
    ui: &egui::Ui,
    active: &mut Surface,
    inputs: &TrayInputs<'_>,
    origin: egui::Pos2,
    quad: f32,
) -> bool {
    let cell = quad / 2.0;
    let mut routed = false;
    for (q, items) in STATUS_QUADS.iter().enumerate() {
        let quad_top = origin.y + q as f32 * quad;
        for (i, &item) in items.iter().enumerate() {
            let (row, col) = (i / 2, i % 2);
            let rect = egui::Rect::from_min_size(
                egui::pos2(origin.x + col as f32 * cell, quad_top + row as f32 * cell),
                egui::vec2(cell, cell),
            );
            if quad_cell(ui, item, inputs, rect) {
                *active = route(item);
                routed = true;
            }
        }
        // The full outside outline enclosing the whole 2×2 box (all four sides) —
        // a 1px BORDER rule, so each status box reads as fully enclosed like the
        // accent-outlined app-group boxes above it (operator directive; §4 token).
        let box_rect =
            egui::Rect::from_min_size(egui::pos2(origin.x, quad_top), egui::vec2(quad, quad));
        ui.painter().rect_stroke(
            box_rect,
            Style::RADIUS,
            egui::Stroke::new(1.0, Style::BORDER),
            egui::StrokeKind::Inside,
        );
    }
    routed
}

/// One status-quad cell (design #12/#15/#19): the item's live [`IconView`] painted
/// at [`QUAD_ICON`] through the shared [`paint_icon_view`] (glyph · bolt · dot, or
/// the Chat badge in the dot's place), a hover fill only — no tooltip. A click
/// **routes** (returns `true`; the caller sets `active`). `&Ui` + `ui.interact`
/// over the explicit `rect`, so it paints inside the dock's frame.
fn quad_cell(ui: &egui::Ui, item: TrayItem, inputs: &TrayInputs<'_>, rect: egui::Rect) -> bool {
    let view = icon_view(item, inputs);
    let response = ui.interact(rect, quad_cell_id(item), egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(QUAD_ICON, QUAD_ICON));
    paint_icon_view(ui, &painter, &view, icon_rect, QUAD_ICON);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::{
        battery_fill_icon, battery_tone, charging, chat_badge, chat_dot, icon_view, peers_dot,
        quad_cell_id, route, signal_dot, status_dot, status_quads, system_pack, volume_view,
        TrayInputs, TrayItem, STATUS_QUADS,
    };
    use crate::chrome::MeshSummary;
    use crate::dock::{Surface, DOCK_W};
    use mde_cosmic_applet::LighthouseHealth;
    use mde_egui::egui;
    use mde_egui::Style;
    use mde_seat::{
        Backend, Battery, BatteryKind, BatteryState, BtAdapter, BtStatus, MixerStatus, MixerStrip,
        Probe, SeatSnapshot, StripOrigin,
    };
    use mde_theme::brand::icons::IconId;

    /// A typed-absent probe of any section (the honest build-host state).
    fn absent<T>() -> Probe<T> {
        Probe::Absent {
            backend: Backend::PipeWire,
            reason: "PipeWire is not available: test".to_string(),
        }
    }

    /// An all-absent seat snapshot the per-section fixtures override.
    fn seat() -> SeatSnapshot {
        SeatSnapshot {
            bluetooth: absent(),
            batteries: absent(),
            on_ac: absent(),
            power: absent(),
            power_profile: absent(),
            charge_limit: absent(),
            lid: absent(),
            displays: absent(),
            backlights: absent(),
            mixer: absent(),
            ddc: absent(),
        }
    }

    /// A master mixer strip at a chosen mute state.
    fn mixer(muted: bool) -> MixerStatus {
        MixerStatus {
            master: MixerStrip {
                id: "master".to_string(),
                name: "Master".to_string(),
                origin: StripOrigin::HostSession,
                volume: 40,
                muted,
            },
            strips: Vec::new(),
        }
    }

    /// One internal system pack at a chosen charge/state.
    fn pack(percentage: f64, state: BatteryState, power_supply: bool) -> Battery {
        Battery {
            model: "BAT0".to_string(),
            kind: BatteryKind::Internal,
            percentage,
            state,
            power_supply,
            time_to_empty: None,
            time_to_full: None,
            energy_rate: None,
        }
    }

    /// One Bluetooth adapter at a chosen radio power.
    fn adapter(powered: bool) -> BtAdapter {
        BtAdapter {
            path: "/org/bluez/hci0".into(),
            name: "hci0".into(),
            powered,
            discovering: false,
            discoverable: false,
            pairable: false,
        }
    }

    /// A seen mesh summary at a chosen presence/health shape.
    fn mesh(online: usize, total: usize, health: LighthouseHealth) -> MeshSummary {
        MeshSummary {
            peers_total: total,
            peers_online: online,
            health,
            seen: true,
        }
    }

    /// Inputs over a chosen mesh + seat (no unread, no session).
    fn inputs<'a>(mesh: &'a MeshSummary, seat: Option<&'a SeatSnapshot>) -> TrayInputs<'a> {
        TrayInputs {
            mesh,
            seat,
            unread: 0,
            session_active: false,
        }
    }

    // ── the battery fill ladder + bolt + dot (lock W8) ───────────────────────

    #[test]
    fn battery_fill_ladder_maps_charge_to_the_five_glyphs() {
        for (pct, icon) in [
            (0.0, IconId::BatteryEmpty),
            (12.0, IconId::BatteryEmpty),
            (12.5, IconId::BatteryQuarter),
            (25.0, IconId::BatteryQuarter),
            (37.5, IconId::BatteryHalf),
            (50.0, IconId::BatteryHalf),
            (62.5, IconId::BatteryThreeQuarter),
            (75.0, IconId::BatteryThreeQuarter),
            (87.5, IconId::BatteryFull),
            (100.0, IconId::BatteryFull),
        ] {
            assert_eq!(battery_fill_icon(pct), icon, "{pct}% → wrong fill glyph");
        }
    }

    #[test]
    fn the_bolt_overlays_only_while_taking_charge() {
        assert!(charging(BatteryState::Charging));
        assert!(charging(BatteryState::PendingCharge));
        assert!(!charging(BatteryState::Discharging));
        assert!(!charging(BatteryState::FullyCharged));
        assert!(!charging(BatteryState::Empty));
    }

    #[test]
    fn battery_dot_reads_amber_low_and_red_critical_only_while_draining() {
        // Charging / full → OK, regardless of charge.
        assert_eq!(
            battery_tone(&pack(9.0, BatteryState::Charging, true)),
            Style::OK
        );
        assert_eq!(
            battery_tone(&pack(100.0, BatteryState::FullyCharged, true)),
            Style::OK
        );
        // Draining: healthy → dim, low (<20) → WARN, critical (≤5) → DANGER.
        assert_eq!(
            battery_tone(&pack(72.0, BatteryState::Discharging, true)),
            Style::TEXT_DIM
        );
        assert_eq!(
            battery_tone(&pack(12.0, BatteryState::Discharging, true)),
            Style::WARN
        );
        assert_eq!(
            battery_tone(&pack(3.0, BatteryState::Discharging, true)),
            Style::DANGER
        );
        // An empty pack → DANGER too.
        assert_eq!(
            battery_tone(&pack(0.0, BatteryState::Empty, true)),
            Style::DANGER
        );
    }

    #[test]
    fn battery_view_folds_the_system_pack_over_a_fuller_peripheral() {
        // The `PowerSupply` cell is summarised even though a peripheral is
        // fuller — a mouse must not mask the low system pack.
        let cells = vec![
            pack(95.0, BatteryState::Discharging, false), // peripheral mouse
            pack(15.0, BatteryState::Discharging, true),  // the system pack, low
        ];
        let chosen = system_pack(&cells).expect("a pack is chosen");
        assert!((chosen.percentage - 15.0).abs() < f64::EPSILON);

        let mut s = seat();
        s.batteries = Probe::Present(cells);
        let m = MeshSummary::default();
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryQuarter, "15% → quarter fill");
        assert!(!view.bolt);
        assert_eq!(view.dot, Style::WARN);
    }

    #[test]
    fn battery_view_is_honestly_dim_when_absent_or_empty() {
        let m = MeshSummary::default();
        // No snapshot yet / absent backend / no pack — the dim empty outline.
        let view = icon_view(TrayItem::Battery, &inputs(&m, None));
        assert_eq!((view.glyph, view.bolt), (IconId::BatteryEmpty, false));
        assert_eq!(view.dot, Style::TEXT_DIM);
        let s = seat(); // batteries Absent
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryEmpty);
        let mut s = seat();
        s.batteries = Probe::Present(Vec::new()); // a desktop with no pack
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(
            (view.glyph, view.dot),
            (IconId::BatteryEmpty, Style::TEXT_DIM)
        );
    }

    #[test]
    fn a_charging_pack_carries_the_bolt_over_its_fill_glyph() {
        let mut s = seat();
        s.batteries = Probe::Present(vec![pack(80.0, BatteryState::Charging, true)]);
        let m = MeshSummary::default();
        let view = icon_view(TrayItem::Battery, &inputs(&m, Some(&s)));
        assert_eq!(view.glyph, IconId::BatteryThreeQuarter);
        assert!(view.bolt, "a charging pack overlays the bolt");
        assert_eq!(view.dot, Style::OK);
    }

    // ── volume / bluetooth (seat folds) ──────────────────────────────────────

    #[test]
    fn volume_swaps_to_the_muted_glyph_with_a_warn_dot() {
        let mut s = seat();
        s.mixer = Probe::Present(mixer(true));
        assert_eq!(volume_view(Some(&s)), (IconId::VolumeMuted, Style::WARN));
        s.mixer = Probe::Present(mixer(false));
        assert_eq!(volume_view(Some(&s)), (IconId::Volume, Style::OK));
        // Absent mixer / pre-poll — the plain glyph, dim (never a fake level).
        assert_eq!(
            volume_view(Some(&seat())),
            (IconId::Volume, Style::TEXT_DIM)
        );
        assert_eq!(volume_view(None), (IconId::Volume, Style::TEXT_DIM));
    }

    #[test]
    fn bluetooth_dot_is_ok_only_while_an_adapter_is_powered() {
        let mut s = seat();
        s.bluetooth = Probe::Present(BtStatus {
            adapters: vec![adapter(true)],
            devices: Vec::new(),
        });
        let m = MeshSummary::default();
        assert_eq!(
            icon_view(TrayItem::Bluetooth, &inputs(&m, Some(&s))).dot,
            Style::OK
        );
        s.bluetooth = Probe::Present(BtStatus {
            adapters: vec![adapter(false)],
            devices: Vec::new(),
        });
        assert_eq!(
            icon_view(TrayItem::Bluetooth, &inputs(&m, Some(&s))).dot,
            Style::TEXT_DIM
        );
        // Absent / pre-poll → dim, and always the small Bluetooth rune.
        let view = icon_view(TrayItem::Bluetooth, &inputs(&m, None));
        assert_eq!(
            (view.glyph, view.dot),
            (IconId::BluetoothSmall, Style::TEXT_DIM)
        );
    }

    // ── the mesh dots (Peers / Status / Signal) ──────────────────────────────

    #[test]
    fn mesh_dots_fold_presence_and_health() {
        // All online + healthy → three OK dots.
        let up = mesh(3, 3, LighthouseHealth::AllHealthy);
        assert_eq!(peers_dot(&up), Style::OK);
        assert_eq!(status_dot(&up), Style::OK);
        assert_eq!(signal_dot(&up), Style::OK);

        // Some away → Peers amber; any peer up keeps Signal OK.
        let some = mesh(2, 3, LighthouseHealth::AllHealthy);
        assert_eq!(peers_dot(&some), Style::WARN);
        assert_eq!(signal_dot(&some), Style::OK);

        // Nobody answers a populated directory → Signal amber "isolated".
        let isolated = mesh(0, 3, LighthouseHealth::Degraded);
        assert_eq!(signal_dot(&isolated), Style::WARN);
        assert_eq!(status_dot(&isolated), Style::DANGER);

        // No lighthouses in view → a dim Status, never a fabricated verdict.
        assert_eq!(
            status_dot(&mesh(1, 1, LighthouseHealth::None)),
            Style::TEXT_DIM
        );

        // Unseen (pre-first-snapshot) → everything dim.
        let unseen = MeshSummary::default();
        assert_eq!(peers_dot(&unseen), Style::TEXT_DIM);
        assert_eq!(status_dot(&unseen), Style::TEXT_DIM);
        assert_eq!(signal_dot(&unseen), Style::TEXT_DIM);
    }

    // ── chat badge ───────────────────────────────────────────────────────────

    #[test]
    fn chat_badge_counts_and_caps_and_the_dot_covers_quiet() {
        assert_eq!(chat_badge(0), None);
        assert_eq!(chat_badge(7), Some("7".to_string()));
        assert_eq!(chat_badge(99), Some("99".to_string()));
        assert_eq!(chat_badge(240), Some("99+".to_string()));
        assert_eq!(chat_dot(0), Style::TEXT_DIM);
        assert_eq!(chat_dot(3), Style::ACCENT);

        let m = MeshSummary::default();
        let view = icon_view(
            TrayItem::Chat,
            &TrayInputs {
                mesh: &m,
                seat: None,
                unread: 120,
                session_active: false,
            },
        );
        assert_eq!(view.glyph, IconId::Chat);
        assert_eq!(view.badge.as_deref(), Some("99+"));
    }

    // ── click routing (lock W7/#15) ──────────────────────────────────────────

    #[test]
    fn every_tray_icon_routes_to_its_owning_surface() {
        for (item, surface) in [
            (TrayItem::Sessions, Surface::Desktop),
            (TrayItem::Chat, Surface::Chat),
            (TrayItem::Bluetooth, Surface::System),
            (TrayItem::Volume, Surface::System),
            (TrayItem::Battery, Surface::System),
            (TrayItem::Status, Surface::MeshView),
            (TrayItem::Signal, Surface::MeshView),
            (TrayItem::Peers, Surface::MeshView),
        ] {
            assert_eq!(route(item), surface, "{item:?} → wrong surface");
        }
    }

    // ── VDOCK-3: the status 2×2 quads (design #6/#8/#15/#19) ──────────────────

    #[test]
    fn the_status_quads_are_two_stacked_2x2_grids() {
        // Design #6 — two quads, four cells each (2×2×2 = eight cells): the seat/
        // Chat cluster over the mesh/session cluster, in the locked order.
        assert_eq!(STATUS_QUADS.len(), 2, "two stacked quads");
        for q in STATUS_QUADS {
            assert_eq!(q.len(), 4, "each quad is a 2×2 grid");
        }
        assert_eq!(
            STATUS_QUADS.iter().flatten().count(),
            8,
            "eight status cells"
        );
        assert_eq!(
            STATUS_QUADS[0],
            [
                TrayItem::Chat,
                TrayItem::Bluetooth,
                TrayItem::Volume,
                TrayItem::Battery
            ],
            "quad 1: Chat · BT · Vol · Batt"
        );
        assert_eq!(
            STATUS_QUADS[1],
            [
                TrayItem::Status,
                TrayItem::Signal,
                TrayItem::Peers,
                TrayItem::Sessions
            ],
            "quad 2: Status · Signal · Peers · Sessions"
        );
    }

    /// Mount the two status quads alone in a `CentralPanel` (the vdock tests'
    /// headless `ctx.run` harness) at the bottom of a `DOCK_W`-wide column, over a
    /// present unmuted mixer, and run one frame feeding `events`. The two quads span
    /// `2 · DOCK_W` down from `origin`.
    fn run_quads(
        ctx: &egui::Context,
        active: &mut Surface,
        unread: usize,
        session_active: bool,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mesh = MeshSummary::default();
        let mut s = seat();
        s.mixer = Probe::Present(mixer(false));
        let inputs = TrayInputs {
            mesh: &mesh,
            seat: Some(&s),
            unread,
            session_active,
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(DOCK_W, 400.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default())
                .show(ctx, |ui| {
                    // Anchor the two-quad span (2 · DOCK_W tall) at the column bottom.
                    let quads_span = 2.0 * DOCK_W;
                    let origin = egui::pos2(0.0, 400.0 - quads_span);
                    let _ = status_quads(ui, active, &inputs, origin, DOCK_W);
                });
        })
    }

    /// A primary-button press/release pair at `pos` (the egui click model:
    /// press one frame, release the next).
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Every text string painted this frame (recursing shape groups). The quad
    /// cells are icon-only, so the only text is the Chat unread badge count.
    fn badge_texts(out: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, acc: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => acc.push(t.galley.text().to_owned()),
                egui::Shape::Vec(v) => {
                    for s in v {
                        walk(s, acc);
                    }
                }
                _ => {}
            }
        }
        let mut acc = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut acc);
        }
        acc
    }

    #[test]
    fn the_quads_lay_out_as_two_stacked_2x2_grids() {
        // Design #6/#8 — the eight cells form two 2×2 grids stacked vertically: in
        // each quad two columns × two rows of equal DOCK_W/2 cells, quad 2 a full
        // quad directly beneath quad 1, spanning the full column width.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::Workbench;
        // Prime two frames so every cell rect settles under its stable id.
        let _ = run_quads(&ctx, &mut active, 0, true, Vec::new());
        let _ = run_quads(&ctx, &mut active, 0, true, Vec::new());

        let rect_of = |item| {
            ctx.read_response(quad_cell_id(item))
                .expect("quad cell registered")
                .rect
        };
        let cell = DOCK_W / 2.0;
        for quad in STATUS_QUADS {
            let (tl, tr, bl, br) = (
                rect_of(quad[0]),
                rect_of(quad[1]),
                rect_of(quad[2]),
                rect_of(quad[3]),
            );
            for r in [tl, tr, bl, br] {
                assert!((r.width() - cell).abs() < 1.0, "cell is DOCK_W/2 wide");
                assert!((r.height() - cell).abs() < 1.0, "cell is DOCK_W/2 tall");
            }
            // Two columns: the left cells share a left edge, the right cells sit one
            // cell over.
            assert!((tl.left() - bl.left()).abs() < 1.0, "left column aligned");
            assert!(
                (tr.left() - tl.right()).abs() < 1.0,
                "right column one cell over"
            );
            assert!((br.left() - tr.left()).abs() < 1.0, "right column aligned");
            // Two rows: the top cells share a top edge, the bottom cells one down.
            assert!((tl.top() - tr.top()).abs() < 1.0, "top row aligned");
            assert!(
                (bl.top() - tl.bottom()).abs() < 1.0,
                "bottom row one cell down"
            );
            assert!((br.top() - bl.top()).abs() < 1.0, "bottom row aligned");
        }
        // Quad 2 is stacked a full quad below quad 1.
        let q1_top = rect_of(STATUS_QUADS[0][0]).top();
        let q2_top = rect_of(STATUS_QUADS[1][0]).top();
        assert!(
            (q2_top - (q1_top + DOCK_W)).abs() < 1.0,
            "quad 2 sits a full quad below quad 1"
        );
        // The quad spans the full column width (two DOCK_W/2 columns).
        let span = rect_of(STATUS_QUADS[0][1]).right() - rect_of(STATUS_QUADS[0][0]).left();
        assert!(
            (span - DOCK_W).abs() < 1.0,
            "the quad spans the column width"
        );
    }

    #[test]
    fn every_status_quad_cell_routes_to_its_owning_surface() {
        // Lock #15 — a click on any of the eight quad cells routes `active` to the
        // surface that owns its state (no flyouts). Mount the quads, read each
        // cell's settled centre by its stable id, click it, and assert the route.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut warm = Surface::Workbench;
        let _ = run_quads(&ctx, &mut warm, 0, true, Vec::new());
        let _ = run_quads(&ctx, &mut warm, 0, true, Vec::new());

        let mut centers: Vec<(TrayItem, egui::Pos2)> = Vec::new();
        for &item in STATUS_QUADS.iter().flatten() {
            let resp = ctx.read_response(quad_cell_id(item));
            assert!(resp.is_some(), "{item:?} quad cell rect not registered");
            centers.push((item, resp.expect("registered above").rect.center()));
        }
        for (item, center) in centers {
            let want = route(item);
            // Start off the target surface so the route is observable.
            let mut active = if want == Surface::Workbench {
                Surface::About
            } else {
                Surface::Workbench
            };
            let _ = run_quads(
                &ctx,
                &mut active,
                0,
                true,
                vec![egui::Event::PointerMoved(center), press(center, true)],
            );
            let _ = run_quads(&ctx, &mut active, 0, true, vec![press(center, false)]);
            assert_eq!(
                active, want,
                "clicking {item:?}'s quad cell routes to {want:?}"
            );
        }
    }

    #[test]
    fn the_chat_quad_cell_carries_the_unread_badge() {
        // Design #19 — the Chat cell's badge counts the CHAT-FIX-2 unread tally +
        // peer messages (the SAME `unread` fold the strip counts). The quad cells
        // are icon-only, so with unread > 0 the ONLY text in the render is the badge
        // count; a quiet (0) tally paints no badge, and the count caps at "99+".
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut active = Surface::Workbench;

        let quiet = run_quads(&ctx, &mut active, 0, false, Vec::new());
        assert!(
            badge_texts(&quiet).is_empty(),
            "a quiet tray paints no badge (the quads are icon-only)"
        );

        let five = run_quads(&ctx, &mut active, 5, false, Vec::new());
        assert_eq!(
            badge_texts(&five),
            vec!["5".to_string()],
            "the Chat cell shows the unread count"
        );

        let flood = run_quads(&ctx, &mut active, 250, false, Vec::new());
        assert_eq!(
            badge_texts(&flood),
            vec!["99+".to_string()],
            "the badge caps at 99+ (the strip's cap)"
        );
    }
}
