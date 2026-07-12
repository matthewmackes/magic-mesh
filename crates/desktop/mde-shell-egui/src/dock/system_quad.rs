//! VDOCK-4 — the **system quad** + Power menu, split out of the dock
//! god-module (pure relocation, no behaviour change).
//!
//! The 2×2 control cluster in the bottom band — Settings · Show-Desktop ·
//! Lock · Power — plus the anchored Lock/Suspend/Reboot/Shutdown Power menu
//! with its typed-arming echo. `use super::*` pulls in the parent's `DockState`,
//! `Surface`, the layout consts (`HAIRLINE_W`, `DOCK_W`, …) and the egui/theme
//! re-exports; only the items the parent render loop + the locked tests call
//! back into are `pub(super)`.

use super::*;

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
pub(super) const SYS_QUAD_ICON: f32 = Style::SP_M + Style::SP_XS / 2.0;

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
pub(super) enum SysCell {
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
pub(super) const SYSTEM_QUAD: [SysCell; 4] = [
    SysCell::Settings,
    SysCell::ShowDesktop,
    SysCell::Lock,
    SysCell::Power,
];

/// One item of the Power cell's menu (design #18). `Lock` drops the curtain (NOT
/// logind's session Lock); the rest drive their real [`PowerVerb`]. Reboot +
/// Shutdown are typed-armed; Lock + Suspend act on a single click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PowerItem {
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
pub(super) const POWER_MENU: [PowerItem; 4] = [
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
    pub(super) const fn power_verb(self) -> Option<PowerVerb> {
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
pub(super) struct PowerMenu {
    /// Whether the anchored popup is open (toggled by the Power cell).
    pub(super) open: bool,
    /// The verb awaiting its typed confirmation (Reboot / Shutdown) + the
    /// operator-typed echo; `None` while the menu shows its top-level verb list.
    pub(super) arming: Option<Arming>,
}

/// A host-down verb mid typed-arming: the verb + the echo the operator types to
/// arm it (the storage surface's arming-echo idiom).
#[derive(Debug)]
pub(super) struct Arming {
    /// The verb this stage will fire once its echo matches.
    pub(super) verb: PowerItem,
    /// The operator-typed echo — must equal [`PowerItem::label`] (case-insensitive)
    /// for [`PowerMenu::armed`] to be `true`.
    pub(super) echo: String,
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
    pub(super) fn close(&mut self) {
        self.open = false;
        self.arming = None;
    }

    /// Enter the typed-arming stage for a host-down verb, with an empty echo.
    pub(super) fn arm(&mut self, verb: PowerItem) {
        self.arming = Some(Arming {
            verb,
            echo: String::new(),
        });
    }

    /// Whether the in-flight arming's echo matches its verb's label — the gate a
    /// Reboot/Shutdown confirm must pass (§7 — a blank / mistyped echo never fires).
    pub(super) fn armed(&self) -> bool {
        self.arming
            .as_ref()
            .is_some_and(|a| a.echo.trim().eq_ignore_ascii_case(a.verb.label()))
    }
}

/// The stable per-cell id of a system-quad cell, so the render + routing are
/// unchanged but the layout is addressable — tests read a cell's settled `Rect`
/// back to click its centre, kept distinct so a system cell never shares an id
/// with a status/picker cell.
pub(super) fn sys_cell_id(cell: SysCell) -> egui::Id {
    egui::Id::new(("vdock-system-quad-cell", cell))
}

/// The stable id of a Power-menu row (design #18), so tests can read its rect back.
pub(super) fn power_item_id(item: PowerItem) -> egui::Id {
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
pub(super) fn system_quad(
    ui: &egui::Ui,
    state: &mut DockState,
    origin: egui::Pos2,
    quad: f32,
) -> bool {
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
    paint_focus_ring(&painter, rect, response.has_focus());
    response.clicked()
}

pub(super) fn sys_cell_tint(cell: SysCell, active: bool, hovered: bool) -> egui::Color32 {
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
    paint_focus_ring(&painter, rect, response.has_focus());
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
