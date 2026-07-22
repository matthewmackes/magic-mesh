//! Airspace — the real-time wardriving surface for Car Mode.
//!
//! A graphically-active RF picture of what's around the vehicle: a **PPI radar
//! scope** with the car at center, a rotating sweep, and network blips placed by
//! **bearing** (direction, relative to heading) and **signal** (distance from
//! center) — WiFi / cellular / Bluetooth, color-coded by type, pulsing as the
//! sweep passes. Beside it a live-now panel grouped by type with bars + dBm, and
//! tap-to-research detail. Real-time only (no history); scans only while open.
//!
//! Production is LIVE-ONLY (WL-UX-007/S1, operator directive 2026-07-22,
//! PLATFORM-INTERFACES P8/Q33): no scanner source exists yet, so the production
//! [`AirspaceState::live`] state holds ZERO contacts and the scope renders a
//! designed honest-empty ("NO SCANNER FEED") rather than fake radar. The future
//! source is the `mackesd` `airspace` worker (MG90 WiFi survey + AT/QMI cell +
//! BT/BLE), which will populate `signals` at this same seam. The rich animated
//! simulation ([`AirspaceState::simulated`]) survives as a cfg-gated test
//! fixture only.

use std::f32::consts::TAU;

use mde_egui::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use mde_egui::Style;

/// A discovered wireless emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    Wifi,
    Cell,
    Bluetooth,
}

impl SignalKind {
    /// The type accent (WiFi cyan, cell mesh-green, BT media-pink).
    pub const fn color(self) -> Color32 {
        match self {
            Self::Wifi => Style::ACCENT_COMMS,
            Self::Cell => Style::ACCENT_MESH,
            Self::Bluetooth => Style::ACCENT_MEDIA,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Wifi => "WiFi",
            Self::Cell => "Cellular",
            Self::Bluetooth => "Bluetooth",
        }
    }
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Cell => "cell-tower",
            Self::Bluetooth => "bluetooth",
        }
    }
}

/// One live signal in the airspace.
#[derive(Clone, Debug)]
pub struct AirspaceSignal {
    pub kind: SignalKind,
    /// BSSID / cell-id / BT MAC — the unique id.
    pub id: String,
    /// SSID / carrier / device name.
    pub name: String,
    /// Signal strength, dBm (strong ≈ -40, weak ≈ -100).
    pub signal_dbm: i32,
    /// Bearing from the vehicle, degrees clockwise from the heading (0 = ahead).
    pub bearing_deg: f32,
    pub channel: Option<u16>,
    /// WiFi encryption (`None` for cell/BT).
    pub encryption: Option<String>,
    /// Security-notable (open / hidden / WEP).
    pub notable: bool,
    /// On the operator's watchlist.
    pub watchlist: bool,
    /// The operator's own mesh gear (marked, alert-suppressed).
    pub own: bool,
    /// Per-signal animation phase so the sim jitters independently.
    phase: f32,
}

impl AirspaceSignal {
    /// The animated live signal (a small time-varying flutter around the base).
    fn live_dbm(&self, t: f32) -> i32 {
        self.signal_dbm + ((t * 1.7 + self.phase).sin() * 3.0) as i32
    }
    /// 0..1 signal fraction (strong→1). -100 dBm→0, -40 dBm→1.
    fn strength(&self, t: f32) -> f32 {
        ((self.live_dbm(t) as f32 + 100.0) / 60.0).clamp(0.0, 1.0)
    }
    /// 5-bar meter count.
    fn bars(&self) -> usize {
        match self.signal_dbm {
            d if d >= -50 => 5,
            d if d >= -65 => 4,
            d if d >= -78 => 3,
            d if d >= -90 => 2,
            d if d >= -100 => 1,
            _ => 0,
        }
    }
}

/// The live airspace picture. Real-time only — repopulated each drive.
#[derive(Clone, Debug)]
pub struct AirspaceState {
    /// Whether Airspace is active (scanning + rendering). Scans only when true.
    pub active: bool,
    pub signals: Vec<AirspaceSignal>,
    pub show_wifi: bool,
    pub show_cell: bool,
    pub show_bt: bool,
    /// Pins ⇄ heatmap for the map overlay (the scope always shows blips).
    pub heatmap: bool,
    /// Currently-selected signal id (instant-research detail card).
    pub selected: Option<String>,
}

impl Default for AirspaceState {
    fn default() -> Self {
        Self::live()
    }
}

impl AirspaceState {
    /// The production airspace: ZERO contacts, scanning idle until the operator
    /// focuses the tab (which arms the seam). With no MG90 airspace worker
    /// wired there is no source, and the scope reads "no scanner feed" — never
    /// fabricated radar contacts. PLATFORM-INTERFACES P8/Q33.
    #[must_use]
    pub fn live() -> Self {
        Self {
            active: false,
            signals: Vec::new(),
            show_wifi: true,
            show_cell: true,
            show_bt: true,
            heatmap: false,
            selected: None,
        }
    }

    /// A rich, plausible airspace fixture. TEST FIXTURE ONLY — compiled solely
    /// for this crate's tests and `sim-fixture` dev builds; no production path
    /// can construct it. ~30 emitters at varied bearings/signals/types.
    #[cfg(any(test, feature = "sim-fixture"))]
    #[must_use]
    pub fn simulated() -> Self {
        // (kind, name, dbm, bearing, chan, enc, notable, watch, own)
        let seed: &[(
            SignalKind,
            &str,
            i32,
            f32,
            Option<u16>,
            Option<&str>,
            bool,
            bool,
            bool,
        )] = &[
            (
                SignalKind::Wifi,
                "MACKES-MESH",
                -44,
                2.0,
                Some(36),
                Some("WPA3"),
                false,
                false,
                true,
            ),
            (
                SignalKind::Wifi,
                "xfinitywifi",
                -71,
                47.0,
                Some(6),
                None,
                true,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "NETGEAR58",
                -63,
                88.0,
                Some(11),
                Some("WPA2"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "<hidden>",
                -66,
                129.0,
                Some(1),
                None,
                true,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "linksys",
                -82,
                156.0,
                Some(6),
                Some("WEP"),
                true,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "ATT-Guest",
                -58,
                201.0,
                Some(44),
                Some("WPA2"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "HOME-5G-2.4",
                -74,
                233.0,
                Some(3),
                Some("WPA2"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "TrackedAP",
                -69,
                271.0,
                Some(9),
                Some("WPA2"),
                false,
                true,
                false,
            ),
            (
                SignalKind::Wifi,
                "CoffeeShop",
                -77,
                312.0,
                Some(6),
                None,
                true,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "SETUP-4471",
                -88,
                341.0,
                Some(11),
                Some("WPA2"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "Verizon LTE",
                -61,
                15.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "AT&T 5G",
                -73,
                96.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "T-Mobile",
                -84,
                178.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "FirstNet",
                -67,
                254.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "Verizon nbr",
                -91,
                318.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "Galaxy S24",
                -59,
                33.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "AirPods Pro",
                -64,
                71.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "Tile",
                -80,
                118.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "Fitbit",
                -76,
                149.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "SYNC 3",
                -48,
                189.0,
                None,
                None,
                false,
                false,
                true,
            ),
            (
                SignalKind::Bluetooth,
                "BLE-Beacon",
                -83,
                227.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "Unknown",
                -90,
                299.0,
                None,
                None,
                false,
                true,
                false,
            ),
            (
                SignalKind::Wifi,
                "Starbucks",
                -70,
                62.0,
                Some(1),
                None,
                true,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "eero-guest",
                -68,
                108.0,
                Some(149),
                Some("WPA3"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "AT&T nbr",
                -95,
                140.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "DIRECT-tv",
                -86,
                165.0,
                Some(6),
                Some("WPA2"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Bluetooth,
                "Bose QC",
                -72,
                210.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "MyHome",
                -55,
                285.0,
                Some(157),
                Some("WPA3"),
                false,
                false,
                false,
            ),
            (
                SignalKind::Cell,
                "T-Mobile nbr",
                -88,
                350.0,
                None,
                None,
                false,
                false,
                false,
            ),
            (
                SignalKind::Wifi,
                "printer-x9",
                -92,
                21.0,
                Some(6),
                None,
                true,
                false,
                false,
            ),
        ];
        let signals = seed
            .iter()
            .enumerate()
            .map(|(i, s)| AirspaceSignal {
                kind: s.0,
                id: format!(
                    "{:02X}:{:02X}:{:02X}",
                    i * 7 % 256,
                    i * 31 % 256,
                    i * 53 % 256
                ),
                name: s.1.to_string(),
                signal_dbm: s.2,
                bearing_deg: s.3,
                channel: s.4,
                encryption: s.5.map(str::to_string),
                notable: s.6,
                watchlist: s.7,
                own: s.8,
                phase: i as f32 * 0.6,
            })
            .collect();
        Self {
            active: true,
            signals,
            show_wifi: true,
            show_cell: true,
            show_bt: true,
            heatmap: false,
            selected: None,
        }
    }

    fn shows(&self, k: SignalKind) -> bool {
        match k {
            SignalKind::Wifi => self.show_wifi,
            SignalKind::Cell => self.show_cell,
            SignalKind::Bluetooth => self.show_bt,
        }
    }

    /// The count of visible in-range emitters (the live counter).
    #[must_use]
    pub fn in_range(&self) -> usize {
        self.signals.iter().filter(|s| self.shows(s.kind)).count()
    }

    /// Toggle a type filter (a keyboard-mappable action target).
    pub fn toggle_kind(&mut self, k: SignalKind) {
        match k {
            SignalKind::Wifi => self.show_wifi = !self.show_wifi,
            SignalKind::Cell => self.show_cell = !self.show_cell,
            SignalKind::Bluetooth => self.show_bt = !self.show_bt,
        }
    }
}

fn finite_or(v: f32, f: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        f
    }
}

/// Render the Airspace surface: the radar scope + a live-now panel beside it.
pub fn airspace_panel(ui: &mut Ui, state: &mut AirspaceState) {
    // Continuous repaint drives the sweep + signal flutter — the "active" look.
    if state.active {
        // ~30 fps animation cadence — smooth sweep/flutter without pinning the CPU
        // (the debug seat is unoptimized; the release build has ample headroom).
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    let t = ui.input(|i| i.time) as f32;
    let full = ui.available_rect_before_wrap();
    if !full.is_finite() || full.width() < 40.0 || full.height() < 40.0 {
        return;
    }
    ui.painter().rect_filled(full, 0.0, Style::BG);

    // Scope on the left (square, ~55% width), live-now panel on the right.
    let scope_w = (full.width() * 0.55).clamp(120.0, full.height() * 1.2);
    let scope_rect = Rect::from_min_size(
        full.min + Vec2::splat(Style::SP_M),
        Vec2::new(
            (scope_w - Style::SP_M * 2.0).max(80.0),
            (full.height() - Style::SP_M * 2.0).max(80.0),
        ),
    );
    paint_radar_scope(ui, scope_rect, state, t);

    let panel_rect = Rect::from_min_max(
        Pos2::new(scope_rect.right() + Style::SP_M, full.top() + Style::SP_M),
        full.max - Vec2::splat(Style::SP_M),
    );
    if panel_rect.width() > 60.0 {
        paint_live_panel(ui, panel_rect, state, t);
    }
}

/// The PPI radar scope — the graphical centerpiece.
fn paint_radar_scope(ui: &mut Ui, rect: Rect, state: &mut AirspaceState, t: f32) {
    let p = ui.painter_at(rect);
    let center = rect.center();
    let radius = (rect.width().min(rect.height()) * 0.5 - 6.0).max(20.0);

    // Scope face + range rings (dim), with a subtle green scope tint.
    p.circle_filled(center, radius, Style::BG);
    for i in 1..=4 {
        let r = radius * (i as f32 / 4.0);
        p.circle_stroke(center, r, Stroke::new(1.0, Style::BORDER));
    }
    // Cross-hairs + bearing cardinals (relative to heading — N/up = ahead).
    p.line_segment(
        [
            Pos2::new(center.x, center.y - radius),
            Pos2::new(center.x, center.y + radius),
        ],
        Stroke::new(1.0, Style::BORDER),
    );
    p.line_segment(
        [
            Pos2::new(center.x - radius, center.y),
            Pos2::new(center.x + radius, center.y),
        ],
        Stroke::new(1.0, Style::BORDER),
    );
    for (lbl, ang) in [
        ("AHEAD", 0.0f32),
        ("R", 90.0),
        ("REAR", 180.0),
        ("L", 270.0),
    ] {
        let a = ang.to_radians() - std::f32::consts::FRAC_PI_2;
        let pos = center + Vec2::new(a.cos(), a.sin()) * (radius - 10.0);
        p.text(
            pos,
            Align2::CENTER_CENTER,
            lbl,
            FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
    }

    // Rotating sweep with a fading trailing wedge (classic radar).
    let sweep = (t * 1.1) % TAU;
    let trail = 0.9f32;
    let steps = 18;
    for i in 0..steps {
        let f = i as f32 / steps as f32;
        let a = sweep - trail * f;
        let dir = Vec2::new(a.cos(), a.sin());
        let alpha = (1.0 - f) * 0.5;
        p.line_segment(
            [center, center + dir * radius],
            Stroke::new(2.0, Style::OK.gamma_multiply(alpha)),
        );
    }
    // The leading sweep edge, bright.
    let lead = Vec2::new(sweep.cos(), sweep.sin());
    p.line_segment(
        [center, center + lead * radius],
        Stroke::new(2.0, Style::OK),
    );

    // Blips: bearing = angle (up = ahead), radius = inverse signal (strong→near).
    let mut hit: Option<String> = None;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for s in state.signals.iter().filter(|s| state.shows(s.kind)) {
        let str_frac = s.strength(t);
        let br = (1.0 - str_frac) * 0.82 + 0.12; // strong = near center
        let a = s.bearing_deg.to_radians() - std::f32::consts::FRAC_PI_2;
        let pos =
            center + Vec2::new(finite_or(a.cos(), 0.0), finite_or(a.sin(), 0.0)) * radius * br;
        // Radar refresh pulse — brighten when the sweep passes the blip's angle.
        let ang_norm = (s.bearing_deg.to_radians()).rem_euclid(TAU);
        let d = (sweep - ang_norm).rem_euclid(TAU);
        let pulse = if d < 0.5 { 1.0 - d * 1.6 } else { 0.0 };
        let base = s.kind.color();
        let sz = 3.0 + str_frac * 4.0 + pulse * 3.0;
        // Glow ring on refresh + for notable/watchlist.
        if pulse > 0.05 {
            p.circle_stroke(pos, sz + 4.0, Stroke::new(1.5, base.gamma_multiply(pulse)));
        }
        if s.watchlist {
            p.circle_stroke(pos, sz + 6.0, Stroke::new(1.5, Style::ACCENT));
        }
        if s.notable {
            p.circle_stroke(pos, sz + 3.0, Stroke::new(1.5, Style::DANGER));
        }
        p.circle_filled(pos, sz, base.gamma_multiply(0.55 + pulse * 0.45));
        // Label strong / notable / watchlist blips.
        if str_frac > 0.55 || s.notable || s.watchlist {
            p.text(
                pos + Vec2::new(sz + 4.0, 0.0),
                Align2::LEFT_CENTER,
                &s.name,
                FontId::proportional(Style::SMALL),
                if s.notable {
                    Style::DANGER
                } else {
                    Style::TEXT
                },
            );
        }
        // Instant-research hit test.
        if let Some(ptr) = pointer {
            if (ptr - pos).length() < sz + 8.0 {
                hit = Some(s.id.clone());
            }
        }
    }
    if ui
        .interact(rect, ui.id().with("airspace-scope"), Sense::click())
        .clicked()
    {
        state.selected = hit;
    }

    // Vehicle at center — a heading chevron pointing up (ahead).
    let ch = 7.0;
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(center.x, center.y - ch),
            Pos2::new(center.x - ch * 0.7, center.y + ch * 0.7),
            Pos2::new(center.x, center.y + ch * 0.3),
            Pos2::new(center.x + ch * 0.7, center.y + ch * 0.7),
        ],
        Style::TEXT_STRONG,
        Stroke::NONE,
    ));

    // Live in-range counter, top-left of the scope.
    p.text(
        rect.left_top() + Vec2::splat(2.0),
        Align2::LEFT_TOP,
        format!("{} IN RANGE", state.in_range()),
        FontId::proportional(Style::BODY),
        Style::OK,
    );

    // Honest empty state: the scope stays armed (sweep runs) but a source-less
    // airspace says so explicitly — "no scanner feed", never fake radar. The
    // MG90 airspace worker is the future source (P8/Q33).
    if state.signals.is_empty() {
        p.text(
            center + Vec2::new(0.0, radius * 0.45),
            Align2::CENTER_CENTER,
            "NO SCANNER FEED",
            FontId::proportional(Style::BODY),
            Style::WARN,
        );
        p.text(
            center + Vec2::new(0.0, radius * 0.45 + Style::SP_M),
            Align2::CENTER_CENTER,
            "MG90 airspace worker not wired",
            FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
    }
}

/// The live-now panel — grouped by type, bars + dBm, with the research detail.
fn paint_live_panel(ui: &mut Ui, rect: Rect, state: &mut AirspaceState, t: f32) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
    child.set_clip_rect(rect);
    let ui = &mut child;

    // Type filter toggles.
    ui.horizontal(|ui| {
        for (k, on) in [
            (SignalKind::Wifi, state.show_wifi),
            (SignalKind::Cell, state.show_cell),
            (SignalKind::Bluetooth, state.show_bt),
        ] {
            let txt = egui::RichText::new(k.label())
                .size(Style::SMALL)
                .color(if on { k.color() } else { Style::TEXT_DIM });
            if ui.selectable_label(on, txt).clicked() {
                state.toggle_kind(k);
            }
        }
    });
    ui.add_space(Style::SP_XS);

    // Instant-research detail card for the selected signal.
    if let Some(sel) = state.selected.clone() {
        if let Some(s) = state.signals.iter().find(|s| s.id == sel) {
            egui::Frame::NONE
                .fill(Style::LAYER_02)
                .inner_margin(Style::SP_S)
                .corner_radius(egui::CornerRadius::same(Style::RADIUS_M as u8))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&s.name)
                            .strong()
                            .size(Style::TITLE)
                            .color(s.kind.color()),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} · {} dBm{}",
                            s.kind.label(),
                            s.id,
                            s.signal_dbm,
                            s.encryption
                                .as_deref()
                                .map(|e| format!(" · {e}"))
                                .unwrap_or_default(),
                        ))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                    );
                    ui.horizontal(|ui| {
                        if s.notable {
                            ui.label(
                                egui::RichText::new("⚠ NOTABLE")
                                    .size(Style::SMALL)
                                    .color(Style::DANGER),
                            );
                        }
                        if s.watchlist {
                            ui.label(
                                egui::RichText::new("★ WATCH")
                                    .size(Style::SMALL)
                                    .color(Style::ACCENT),
                            );
                        }
                        if s.own {
                            ui.label(
                                egui::RichText::new("OWN")
                                    .size(Style::SMALL)
                                    .color(Style::OK),
                            );
                        }
                    });
                });
            ui.add_space(Style::SP_XS);
        }
    }

    // Grouped-by-type live list.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for kind in [SignalKind::Wifi, SignalKind::Cell, SignalKind::Bluetooth] {
                if !state.shows(kind) {
                    continue;
                }
                let mut group: Vec<&AirspaceSignal> =
                    state.signals.iter().filter(|s| s.kind == kind).collect();
                group.sort_by_key(|s| -(s.live_dbm(t)));
                ui.add_space(Style::SP_XS);
                ui.label(
                    egui::RichText::new(format!("{} · {}", kind.label(), group.len()))
                        .size(Style::SMALL)
                        .strong()
                        .color(kind.color()),
                );
                if group.is_empty() {
                    // Honest per-layer empty: absent reads absent (Q33) — the
                    // group header stays so the layer is visibly source-less.
                    ui.label(
                        egui::RichText::new(format!("No {} scanner feed", kind.label()))
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    continue;
                }
                for s in group {
                    let selected = state.selected.as_deref() == Some(s.id.as_str());
                    let resp = ui.add(
                        egui::Button::new("")
                            .min_size(Vec2::new(ui.available_width(), Style::SP_L))
                            .fill(if selected {
                                Style::LAYER_02
                            } else {
                                Color32::TRANSPARENT
                            }),
                    );
                    let r = resp.rect;
                    let painter = ui.painter_at(r);
                    // Signal bars.
                    let bars = s.bars();
                    for b in 0..5 {
                        let bx = r.left() + 2.0 + b as f32 * 4.0;
                        let bh = 3.0 + b as f32 * 2.0;
                        let col = if b < bars {
                            s.kind.color()
                        } else {
                            Style::BORDER
                        };
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(bx, r.bottom() - 2.0 - bh),
                                Pos2::new(bx + 3.0, r.bottom() - 2.0),
                            ),
                            0.0,
                            col,
                        );
                    }
                    painter.text(
                        Pos2::new(r.left() + 26.0, r.center().y),
                        Align2::LEFT_CENTER,
                        &s.name,
                        FontId::proportional(Style::BODY),
                        if s.notable {
                            Style::DANGER
                        } else {
                            Style::TEXT_STRONG
                        },
                    );
                    painter.text(
                        Pos2::new(r.right() - 2.0, r.center().y),
                        Align2::RIGHT_CENTER,
                        format!("{} dBm", s.live_dbm(t)),
                        FontId::proportional(Style::SMALL),
                        Style::TEXT_DIM,
                    );
                    if resp.clicked() {
                        state.selected = Some(s.id.clone());
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_feed_is_rich_and_typed() {
        let a = AirspaceState::simulated();
        assert!(a.signals.len() >= 25, "a rich airspace to render");
        assert!(a.signals.iter().any(|s| s.kind == SignalKind::Wifi));
        assert!(a.signals.iter().any(|s| s.kind == SignalKind::Cell));
        assert!(a.signals.iter().any(|s| s.kind == SignalKind::Bluetooth));
        assert!(a.signals.iter().any(|s| s.notable), "has notable networks");
        assert!(
            a.signals.iter().any(|s| s.watchlist),
            "has watchlist entries"
        );
        assert!(a.signals.iter().any(|s| s.own), "recognizes own gear");
    }

    #[test]
    fn live_airspace_is_empty_and_idle() {
        // WL-UX-007/S1: the production constructor carries ZERO contacts and is
        // idle until the operator focuses the tab — no fabricated radar.
        let a = AirspaceState::live();
        assert!(a.signals.is_empty());
        assert!(!a.active);
        assert_eq!(a.in_range(), 0);
        assert!(a.selected.is_none());
        // Default is the live (honest) state, not the fixture.
        let d = AirspaceState::default();
        assert!(d.signals.is_empty());
    }

    #[test]
    fn type_toggles_filter_the_count() {
        let mut a = AirspaceState::simulated();
        let all = a.in_range();
        a.toggle_kind(SignalKind::Bluetooth);
        assert!(
            a.in_range() < all,
            "hiding a type lowers the in-range count"
        );
        assert!(!a.show_bt);
    }

    #[test]
    fn strength_and_bars_track_dbm() {
        let strong = AirspaceSignal {
            kind: SignalKind::Wifi,
            id: "a".into(),
            name: "x".into(),
            signal_dbm: -45,
            bearing_deg: 0.0,
            channel: None,
            encryption: None,
            notable: false,
            watchlist: false,
            own: false,
            phase: 0.0,
        };
        assert!(strong.strength(0.0) > 0.85);
        assert_eq!(strong.bars(), 5);
    }
}
