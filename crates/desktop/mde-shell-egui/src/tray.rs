//! The taskbar **tray** (NAVBAR-W10-2, locks W2/W6..W11) — the right-justified
//! icon strip on the shell's ONE bar: the `^` overflow chevron, then Sessions
//! (only while a VDI session is live), Chat (unread count badge), Bluetooth,
//! Volume (muted variant when muted), Battery (charge-fill glyph + a bolt
//! overlay while charging), Status (worst-of mesh health), and the stacked
//! HH:MM-over-date clock at the right edge. Signal + Peers sit in the chevron's
//! anchored flyout (lock W10) — the Win10 hidden-icons well.
//!
//! Every icon renders **real state** (§7) folded from the same sources the
//! retired top chrome strip read: the world-readable mesh-status snapshot
//! ([`MeshSummary`], the tray's Peers/Status/Signal dots), the ONE `mde-seat`
//! [`SeatSnapshot`] (Bluetooth / Volume / Battery), and the Chat unread tally.
//! State rides a **tiny corner dot** in the OK/WARN/DANGER tokens while the
//! glyph keeps one tint (lock W9); every click **routes to the owning surface**
//! (lock W7 — plain routing; the Chat/Volume/BT micro-flyouts are W10-4) and
//! there are **no tooltips and no labels anywhere** (lock W6).
//!
//! The folds (glyph pick, dot tone, battery fill ladder, hidden-set membership,
//! Sessions transience, clock text, click routing) are pure — no egui
//! `Context`, no IO — so they're unit-tested directly; the only egui here is
//! the strip layout and the anchored flyout.

use mde_egui::egui::{self, FontId};
use mde_egui::Style;

use mde_cosmic_applet::LighthouseHealth;
use mde_seat::{Battery, BatteryState, Probe, SeatSnapshot};
use mde_theme::brand::icons::IconId;

use crate::chrome::MeshSummary;
use crate::dock::{icon_texture, Surface};

/// The tray glyph edge in logical points — the 16px Win10 tray raster (lock
/// W3). `icon_texture` rasterizes it DPI-crisp at the physical size.
const TRAY_ICON: f32 = Style::SP_M;

/// One tray icon cell's width — the 16px glyph plus breathing room on the 8px
/// grid. The cell fills the bar's height so the whole column is clickable.
const TRAY_CELL_W: f32 = Style::SP_L + Style::SP_XS;

/// The corner status dot's radius (lock W9) — tiny, token-derived (§4).
const DOT_R: f32 = Style::SP_XS / 2.0;

/// Charge (%) below which a **draining** system pack's dot reads amber "low",
/// and at or below which it reads red "critical" (lock W8). A charging or full
/// pack is never amber/red (it's improving); these bite only while the pack is
/// actually discharging. Moved verbatim from the retired chrome strip.
const BATTERY_LOW: f64 = 20.0;
const BATTERY_CRITICAL: f64 = 5.0;

// ─────────────────────────────── the tray model ──────────────────────────────

/// One tray icon slot. The strip/hidden partition ([`strip_items`] /
/// [`HIDDEN`]) and the click routing ([`route`]) are keyed off this — pure and
/// unit-tested. (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate`
/// form for crate-visible items in this private module, like `dock::TASKBAR_H`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayItem {
    /// The live VDI session marker — visible ONLY while a session is
    /// connected/pending (lock W10's transient rule).
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
    /// Mesh reachability — hidden in the chevron flyout by default (lock W10).
    Signal,
    /// Peer directory presence — hidden in the chevron flyout by default.
    Peers,
}

/// The visible strip between the chevron and the clock, left→right, with the
/// transient Sessions slot leading while a VDI session is live (lock W10).
const STRIP: [TrayItem; 5] = [
    TrayItem::Chat,
    TrayItem::Bluetooth,
    TrayItem::Volume,
    TrayItem::Battery,
    TrayItem::Status,
];
const STRIP_WITH_SESSION: [TrayItem; 6] = [
    TrayItem::Sessions,
    TrayItem::Chat,
    TrayItem::Bluetooth,
    TrayItem::Volume,
    TrayItem::Battery,
    TrayItem::Status,
];

/// The chevron flyout's hidden set (lock W10): Signal + Peers by default (the
/// compact-mode fold-in is W10-5).
const HIDDEN: [TrayItem; 2] = [TrayItem::Signal, TrayItem::Peers];

/// The visible strip for this frame — Sessions leads only while a VDI session
/// is active; the fixed Chat · BT · Volume · Battery · Status run follows.
const fn strip_items(session_active: bool) -> &'static [TrayItem] {
    if session_active {
        &STRIP_WITH_SESSION
    } else {
        &STRIP
    }
}

/// Where a tray icon click lands (lock W7): every icon jumps to the surface
/// that owns its state — seat hardware to System, mesh telemetry to the Mesh
/// Map, Chat to Chat, the live session to Desktop. The clock routes to System
/// in [`clock_cell`]'s caller.
const fn route(item: TrayItem) -> Surface {
    match item {
        TrayItem::Sessions => Surface::Desktop,
        TrayItem::Chat => Surface::Chat,
        TrayItem::Bluetooth | TrayItem::Volume | TrayItem::Battery => Surface::System,
        TrayItem::Status | TrayItem::Signal | TrayItem::Peers => Surface::MeshView,
    }
}

// ─────────────────────────────── the pure folds ──────────────────────────────

/// Everything the tray folds from, bundled so the `main → dock::taskbar → tray`
/// call chain hands one immutable view of the frame's live state.
pub struct TrayInputs<'a> {
    /// The mesh-status snapshot fold — the same [`crate::chrome::ChromeState`]
    /// poll product the retired strip read (one poll, no second reader).
    pub mesh: &'a MeshSummary,
    /// The ONE `mde-seat` snapshot (Bluetooth / Volume / Battery), `None`
    /// before the System state's first poll lands.
    pub seat: Option<&'a SeatSnapshot>,
    /// The whole-mesh Chat unread tally (folded alerts + clips + messages).
    pub unread: usize,
    /// `true` while a VDI session is connected/pending — the Sessions slot's
    /// transient-visibility signal (lock W10).
    pub session_active: bool,
}

/// One resolved tray icon: the glyph to draw, its corner-dot tone, whether the
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

/// Fold one tray item's live view from the frame's inputs — the single
/// glyph/dot/bolt/badge authority the strip AND the flyout render through.
fn icon_view(item: TrayItem, inputs: &TrayInputs<'_>) -> IconView {
    let plain = |glyph: IconId, dot: egui::Color32| IconView {
        glyph,
        dot,
        bolt: false,
        badge: None,
    };
    match item {
        // Visible only while a session is live, so its dot is honestly OK.
        TrayItem::Sessions => plain(IconId::Sessions, Style::OK),
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

/// The stacked clock's two lines (lock W11): wall-clock `HH:MM` over the civil
/// `YYYY-MM-DD` date, UTC — the same no-time-crate calendar math the Chat
/// timeline uses ([`crate::chat::civil_from_days`]), so the shell never claims
/// a local zone it can't know.
fn clock_lines(unix_secs: i64) -> (String, String) {
    let tod = unix_secs.rem_euclid(86_400);
    let (year, month, day) = crate::chat::civil_from_days(unix_secs.div_euclid(86_400));
    (
        format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60),
        format!("{year:04}-{month:02}-{day:02}"),
    )
}

// ─────────────────────────────── the tray strip ──────────────────────────────

/// The tray's per-frame state: whether the chevron flyout is open. Everything
/// else is folded fresh from [`TrayInputs`] each frame.
#[derive(Default)]
pub struct TrayState {
    /// The `^` overflow flyout is showing (toggled by the chevron, closed by a
    /// routed click or a click elsewhere).
    flyout_open: bool,
}

/// Render the right-justified tray into a right-to-left `ui` (the taskbar's
/// trailing layout): clock · Status · Battery · Volume · Bluetooth · Chat ·
/// [Sessions] · the `^` chevron + its flyout. Every icon click routes `active`
/// to the owning surface (lock W7); returns `true` when any click routed this
/// frame so the shell can surface the body behind a session.
pub fn tray(
    ui: &mut egui::Ui,
    state: &mut TrayState,
    active: &mut Surface,
    inputs: &TrayInputs<'_>,
) -> bool {
    // The stacked clock at the right edge (lock W11); a click opens System.
    let clock_clicked = clock_cell(ui);
    if clock_clicked {
        *active = Surface::System;
        state.flyout_open = false;
    }
    let mut routed = clock_clicked;

    // The visible strip — painted right→left, so iterate the left→right order
    // reversed; Status lands beside the clock, Chat (or Sessions) leftmost.
    for item in strip_items(inputs.session_active).iter().rev() {
        let view = icon_view(*item, inputs);
        if icon_cell(ui, &view, egui::vec2(TRAY_CELL_W, ui.available_height())) {
            *active = route(*item);
            state.flyout_open = false;
            routed = true;
        }
    }

    // The `^` overflow chevron heads the tray (lock W10); its anchored flyout
    // holds the hidden Signal + Peers icons.
    let chevron = chevron_cell(ui);
    if chevron.clicked() {
        state.flyout_open = !state.flyout_open;
    }
    if state.flyout_open {
        if let Some(target) = flyout(ui.ctx(), chevron.rect, inputs, chevron.clicked(), state) {
            *active = target;
            state.flyout_open = false;
            routed = true;
        }
    }
    routed
}

/// One tray icon cell: hover fill only — NO tooltip (lock W6) — the 16px glyph
/// at one tint (lock W9), the bolt overlay, and the corner dot (or the Chat
/// badge in its place). Returns `true` on a click.
fn icon_cell(ui: &mut egui::Ui, view: &IconView, size: egui::Vec2) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(TRAY_ICON, TRAY_ICON));
    if let Some(tex) = icon_texture(ui.ctx(), view.glyph, TRAY_ICON, Style::TEXT_DIM) {
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    // The charging bolt overlays the fill glyph in the same rect (lock W8),
    // brighter than the outline so it reads at 16px.
    if view.bolt {
        if let Some(tex) = icon_texture(ui.ctx(), IconId::BatteryBolt, TRAY_ICON, Style::TEXT) {
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
    response.clicked()
}

/// The `^` chevron cell — glyph only, hover fill, no dot (it carries no state;
/// the hidden icons behind it carry their own).
fn chevron_cell(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(TRAY_CELL_W, ui.available_height()),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    if let Some(tex) = icon_texture(ui.ctx(), IconId::ChevronUp, TRAY_ICON, Style::TEXT_DIM) {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(TRAY_ICON, TRAY_ICON));
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), icon_rect.size()))
            .paint_at(ui, icon_rect);
    }
    response
}

/// The stacked clock (lock W11): `HH:MM` over `YYYY-MM-DD` in small token
/// text, right edge, hover fill, no tooltip. Returns `true` on a click (the
/// caller routes to System). The repaint heartbeat rides the chrome poll's
/// shared cadence, so the minute flip surfaces without input.
fn clock_cell(ui: &mut egui::Ui) -> bool {
    // A small margin keeps the clock off the screen's right edge (RTL: the
    // first space paints rightmost).
    ui.add_space(Style::SP_S);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let (time, date) = clock_lines(now);
    let font = FontId::proportional(Style::SMALL);
    let time_galley = ui.fonts(|f| f.layout_no_wrap(time, font.clone(), Style::TEXT));
    let date_galley = ui.fonts(|f| f.layout_no_wrap(date, font, Style::TEXT_DIM));
    let (time_size, date_size) = (time_galley.size(), date_galley.size());

    let w = time_size.x.max(date_size.x) + Style::SP_S;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(w, ui.available_height()), egui::Sense::click());
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let gap = Style::SP_XS / 2.0;
    let top = rect.center().y - (time_size.y + gap + date_size.y) / 2.0;
    painter.galley(
        egui::pos2(rect.center().x - time_size.x / 2.0, top),
        time_galley,
        Style::TEXT,
    );
    painter.galley(
        egui::pos2(rect.center().x - date_size.x / 2.0, top + time_size.y + gap),
        date_galley,
        Style::TEXT_DIM,
    );
    response.clicked()
}

/// The chevron's anchored flyout (lock W10): a small popup grid of the hidden
/// icons (Signal + Peers), floated just above the chevron on the Foreground
/// order. A click on a hidden icon returns its routed surface; a click
/// anywhere else closes the flyout (the chevron's own click already toggled).
fn flyout(
    ctx: &egui::Context,
    anchor: egui::Rect,
    inputs: &TrayInputs<'_>,
    chevron_clicked: bool,
    state: &mut TrayState,
) -> Option<Surface> {
    /// The flyout's slot count — `HIDDEN.len()` as layout math (pinned by a
    /// unit test so the two can't drift).
    const SLOTS: f32 = 2.0;
    let cell = Style::SP_XL;
    // Right-align the padded panel to the chevron; float it a hair above the
    // bar (the bg pad expands SP_S beyond the content on every side).
    let pos = egui::pos2(
        (anchor.right() - SLOTS.mul_add(cell, Style::SP_S))
            .max(ctx.screen_rect().left() + Style::SP_XS),
        anchor.top() - Style::SP_XS - Style::SP_S - cell,
    );

    let mut routed = None;
    let area = egui::Area::new(egui::Id::new("w10-tray-flyout"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            // Reserve a slot so the panel background paints BEHIND the icons
            // (the keyboard.rs overlay idiom).
            let bg = ui.painter().add(egui::Shape::Noop);
            let inner = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    ui.horizontal(|ui| {
                        for item in HIDDEN {
                            let view = icon_view(item, inputs);
                            if icon_cell(ui, &view, egui::vec2(cell, cell)) {
                                routed = Some(route(item));
                            }
                        }
                    });
                })
                .response
                .rect
                .expand(Style::SP_S);
            ui.painter().set(
                bg,
                egui::Shape::rect_filled(inner, Style::RADIUS, Style::SURFACE),
            );
            ui.painter().rect_stroke(
                inner,
                Style::RADIUS,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
        });

    // Click-away dismiss — unless this frame's click was the chevron's own
    // toggle (which already decided the open state).
    if routed.is_none() && !chevron_clicked && area.response.clicked_elsewhere() {
        state.flyout_open = false;
    }
    routed
}

#[cfg(test)]
mod tests {
    use super::{
        battery_fill_icon, battery_tone, charging, chat_badge, chat_dot, clock_lines, icon_view,
        peers_dot, route, signal_dot, status_dot, strip_items, system_pack, volume_view,
        TrayInputs, TrayItem, HIDDEN,
    };
    use crate::chrome::MeshSummary;
    use crate::dock::Surface;
    use mde_cosmic_applet::LighthouseHealth;
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
        let adapter = |powered| BtAdapter {
            path: "/org/bluez/hci0".into(),
            name: "hci0".into(),
            powered,
            discovering: false,
            discoverable: false,
            pairable: false,
        };
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

    // ── the strip / hidden partition + Sessions transience (lock W10) ────────

    #[test]
    fn signal_and_peers_are_hidden_behind_the_chevron() {
        assert_eq!(HIDDEN, [TrayItem::Signal, TrayItem::Peers]);
        for item in HIDDEN {
            assert!(
                !strip_items(false).contains(&item) && !strip_items(true).contains(&item),
                "{item:?} must live in the flyout, not the strip"
            );
        }
    }

    #[test]
    fn sessions_is_transient_on_a_live_vdi_session() {
        // No session → the fixed five: Chat · BT · Volume · Battery · Status.
        assert_eq!(
            strip_items(false),
            [
                TrayItem::Chat,
                TrayItem::Bluetooth,
                TrayItem::Volume,
                TrayItem::Battery,
                TrayItem::Status,
            ]
        );
        // A live session leads with the Sessions icon; the rest is unchanged.
        assert_eq!(strip_items(true)[0], TrayItem::Sessions);
        assert_eq!(&strip_items(true)[1..], strip_items(false));
        // And it wears the Sessions glyph with an honest OK dot.
        let m = MeshSummary::default();
        let view = icon_view(
            TrayItem::Sessions,
            &TrayInputs {
                mesh: &m,
                seat: None,
                unread: 0,
                session_active: true,
            },
        );
        assert_eq!((view.glyph, view.dot), (IconId::Sessions, Style::OK));
    }

    #[test]
    fn the_flyout_slot_count_matches_the_hidden_set() {
        // `flyout`'s SLOTS layout constant is 2.0 — pinned to the hidden set's
        // size so the panel can't under/over-allocate if the set grows.
        assert_eq!(HIDDEN.len(), 2);
    }

    // ── click routing (lock W7) ──────────────────────────────────────────────

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

    // ── the clock (lock W11) ─────────────────────────────────────────────────

    #[test]
    fn clock_stacks_hh_mm_over_the_civil_date() {
        assert_eq!(
            clock_lines(0),
            ("00:00".to_string(), "1970-01-01".to_string())
        );
        // 2020-01-01T13:05:59Z.
        assert_eq!(
            clock_lines(1_577_836_800 + 13 * 3600 + 5 * 60 + 59),
            ("13:05".to_string(), "2020-01-01".to_string())
        );
    }
}
