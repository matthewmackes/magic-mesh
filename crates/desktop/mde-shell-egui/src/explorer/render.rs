//! EXPLORER-3 render helpers — the **leaf drawing surface** split out of the
//! Discovery hero-card god-module (pure relocation, no behaviour change).
//!
//! Every item here is a stateless painter/row/label helper the parent
//! `ExplorerState` render loop calls: the self-empty placeholder, the focus
//! ring, filter/edge chips, the filmstrip thumbnails + dividers, the zoomable
//! **mosaic** tiles/headers + its `Cluster`/`GridDir` grid geometry, the **IPAM**
//! table cells/rows/bars, the **hero card** + telemetry sparklines, and the
//! accesskit a11y label builders.
//!
//! `use super::*` pulls in the parent's `Unit`/`Edge`/`IpamPrefix`/… wire mirrors,
//! the layout constants, and the egui re-exports; as a child module it reads the
//! parent's private types/fields directly, so only the items the parent calls
//! back into are `pub(super)`.

use super::*;
use mde_egui::style::Elevation;

/// Synthesise this node's own hero unit for the honest empty state (#23) — a real
/// self-reference (hostname, in-mesh), never a faked peer; health stays unknown
/// (the ring spins "discovering") until a real mirror lands.
pub(super) fn self_placeholder(host: &str) -> Unit {
    Unit {
        id: peer_self_id(host),
        kind: UnitKind::Peer,
        name: host.to_string(),
        reachability: Reachability::InMesh,
        address: None,
        health: None,
        telemetry: None,
        mesh: None,
        first_seen_ms: 0,
        last_seen_ms: 0,
        extras: UnitExtras::default(),
    }
}

// ─────────────────────────── render helpers ───────────────────────────

/// The ONE high-contrast keyboard/D-pad **focus ring** (EXPLORER-18, O11):
/// every navigable element — a mosaic tile, a filmstrip thumb, a search hit
/// row — paints its selection through this single stroke, so "where am I"
/// reads identically across the whole surface and can never fork per call
/// site (§4). Delegates to the **shared** platform painter
/// [`mde_egui::focus::paint_focus_ring`] (the 2px `ACCENT_HI` token, design
/// lock #5), so the Explorer's ring is the identical indicator every other
/// shell surface wears. Painted last, over the element's own frame, so the
/// ring is never buried under a category tint.
pub(super) fn focus_ring(painter: &egui::Painter, rect: Rect) {
    mde_egui::focus::paint_focus_ring(painter, rect, true);
}

/// The soft **depth shadow** a raised Explorer card casts (§4 depth tokens,
/// lock #2): at rest the card sits at [`Elevation::Raised`]; a hover progress
/// `t` ∈ `0..=1` eases it toward [`Elevation::Overlay`] — the hover-lift
/// micro-interaction, expressed purely in shadow depth. Every field is a
/// numeric blend of the two shared tokens and the umbra is gamma-blended
/// between the tokens' own umbras, so **no** colour (or duration) is minted
/// here and the seam stays unit-testable without a painter.
pub(super) fn raise_shadow(t: f32) -> egui::Shadow {
    let rest = Elevation::Raised.shadow();
    let lift = Elevation::Overlay.shadow();
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    egui::Shadow {
        offset: [
            lerp(rest.offset[0], lift.offset[0]).round() as i8,
            lerp(rest.offset[1], lift.offset[1]).round() as i8,
        ],
        blur: lerp(rest.blur, lift.blur).round() as u8,
        spread: lerp(rest.spread, lift.spread).round() as u8,
        color: rest.umbra.lerp_to_gamma(lift.umbra, t),
    }
}

/// Paint the [`raise_shadow`] behind a raised card — called **before** the
/// card's fill so the depth reads as a cast shadow under the card, never a
/// wash over it. The hover progress comes from the reduce-motion-aware
/// [`Motion::animate`] on the shared [`Motion::FAST`] cadence, shaped by
/// [`Motion::hover_lift`]. Paint-only: the card's allocated rect is untouched,
/// so hovering never shifts layout.
pub(super) fn raise(painter: &egui::Painter, rect: Rect, id: impl std::hash::Hash, hovered: bool) {
    let t = Motion::hover_lift(Motion::animate(painter.ctx(), id, hovered, Motion::FAST));
    painter.add(raise_shadow(t).as_shape(rect, Style::RADIUS));
}

/// A Carbon filter/nav pill; returns whether it was clicked. Active = accent
/// fill; inactive = surface with a dim border and the `rest` label tone (all §4
/// tokens). The category chips pass their O8 accent as `rest` (EXPLORER-15) so
/// a chip speaks its category identity even when not selected; every other
/// chip passes the plain `TEXT` tone.
pub(super) fn chip(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    accent: Color32,
    rest: Color32,
) -> bool {
    let text = RichText::new(label)
        .size(Style::SMALL)
        .color(if active { Style::BG } else { rest });
    let button = egui::Button::new(text)
        .fill(if active { accent } else { Style::SURFACE })
        .stroke(Stroke::new(
            1.0,
            if active { accent } else { Style::BORDER },
        ));
    ui.add(button).clicked()
}

/// One ranked search hit row (EXPLORER-14): a mini kind glyph + the unit's name,
/// type badge, and reachability/address line; the keyboard-selected row wears the
/// accent frame (Enter jumps it). Returns whether it was clicked (the jump).
pub(super) fn search_hit_row(ui: &mut egui::Ui, unit: &Unit, selected: bool) -> bool {
    let cat = unit.kind.category();
    // Reserve the band slot so it paints BEHIND the row content (the IPAM idiom).
    let band = ui.painter().add(egui::Shape::Noop);
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            ui.set_min_width(ui.available_width());
            ui.add_space(Style::SP_S);
            let (glyph_rect, _) = ui.allocate_exact_size(Vec2::splat(Style::SP_M), Sense::hover());
            paint_kind_glyph(
                ui.painter(),
                glyph_rect.center(),
                Style::SP_M * 0.42,
                unit.kind,
                cat.accent(),
            );
            ui.label(
                RichText::new(&unit.name)
                    .size(Style::BODY)
                    .color(Style::TEXT),
            );
            ui.label(
                RichText::new(unit.kind.label())
                    .size(Style::SMALL)
                    .color(cat.accent()),
            );
            ui.label(
                RichText::new(reachability_line(
                    &unit.reachability,
                    unit.address.as_deref(),
                ))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
            );
        })
        .response
        .interact(Sense::click());
    let fill = if resp.hovered() {
        Style::SURFACE_HI
    } else if selected {
        Style::SURFACE
    } else {
        Style::BG
    };
    ui.painter().set(
        band,
        egui::Shape::rect_filled(resp.rect, Style::RADIUS * 0.5, fill),
    );
    if selected {
        // The keyboard selection wears the shared high-contrast ring
        // (EXPLORER-18, O11) — the same ring as a mosaic tile / filmstrip thumb.
        focus_ring(ui.painter(), resp.rect);
    }
    // a11y-05 — the search hit's accesskit node (name + kind/reachability). A
    // hit row carries no pin/mark set.
    install_unit_accessibility(ui.ctx(), resp.id, unit, false, false, resp.rect);
    resp.clicked()
}

/// One edge jump chip (EXPLORER-8): a mini kind glyph + the neighbour's name in a
/// clickable pill, the border tinted with the neighbour's category accent (the
/// EXPLORER-15 / PICKER category-accent language, §4 tokens — no raw hex). Returns
/// whether it was clicked (the hero-focus jump). Hand-painted (a glyph beside text)
/// rather than an `egui::Button` so the procedural kind glyph rides inside.
pub(super) fn edge_chip(ui: &mut egui::Ui, chip: &ChipItem) -> bool {
    let accent = chip.kind.category().accent();
    let galley = ui.painter().layout_no_wrap(
        truncate(&chip.name, 18),
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    let glyph = Style::SP_M;
    let pad = Style::SP_S;
    let gap = Style::SP_XS;
    let w = pad + glyph + gap + galley.size().x + pad;
    let h = glyph.max(galley.size().y) + Style::SP_XS * 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        Style::RADIUS,
        if hovered {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        },
    );
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        Stroke::new(1.0, if hovered { accent } else { Style::BORDER }),
        StrokeKind::Inside,
    );
    paint_kind_glyph(
        painter,
        egui::pos2(rect.min.x + pad + glyph * 0.5, rect.center().y),
        glyph * 0.42,
        chip.kind,
        accent,
    );
    let text_h = galley.size().y;
    painter.galley(
        egui::pos2(
            rect.min.x + pad + glyph + gap,
            rect.center().y - text_h * 0.5,
        ),
        galley,
        Style::TEXT,
    );
    let resp = resp.on_hover_text(&chip.name);
    // a11y-05 — the edge jump-chip's accesskit node: the neighbour's name +
    // its kind (the two facts the pill paints).
    install_cell_accessibility(
        ui.ctx(),
        resp.id,
        chip.name.clone(),
        format!("{} \u{00B7} neighbour", chip.kind.label()),
        resp.rect,
    );
    resp.clicked()
}

/// A thin vertical cluster divider + label between filmstrip sections (#8, plus
/// the O9 Pinned run).
pub(super) fn filmstrip_divider(ui: &mut egui::Ui, cluster: Cluster) {
    ui.vertical(|ui| {
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(cluster.label())
                .size(Style::SMALL)
                .color(cluster.accent()),
        );
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(Style::SP_XS, THUMB_H * 0.6), Sense::hover());
        ui.painter().line_segment(
            [rect.center_top(), rect.center_bottom()],
            Stroke::new(1.0, Style::BORDER),
        );
    });
}

/// One filmstrip thumbnail — a mini glyph + status dot + truncated name (+ the
/// O9 pin marker); the focused thumb wears the shared high-contrast
/// [`focus_ring`] (EXPLORER-18, O11). Returns whether it was clicked (#6 jump)
/// and whether it was right-clicked (the O9 pin toggle).
pub(super) fn thumbnail(
    ui: &mut egui::Ui,
    unit: &Unit,
    focused: bool,
    pinned: bool,
) -> (bool, bool) {
    let cat = unit.kind.category();
    let resp = ui
        .scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            ui.set_min_size(Vec2::new(THUMB_W, THUMB_H));
            let rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(THUMB_W, THUMB_H));
            let hovered = ui.rect_contains_pointer(rect);
            let border = if hovered {
                Style::ACCENT
            } else {
                Style::BORDER
            };
            // The shared raised-card depth (§4): the thumb rests at Raised and
            // hover-lifts toward Overlay — cast under the fill, no layout shift.
            raise(
                ui.painter(),
                rect,
                ("explorer-thumb-raise", &unit.id),
                hovered,
            );
            ui.painter()
                .rect_filled(rect, Style::RADIUS, Style::SURFACE);
            ui.painter().rect_stroke(
                rect,
                Style::RADIUS,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            // The focused thumb wears the shared high-contrast focus ring
            // (EXPLORER-18, O11) — in hero mode the filmstrip IS where the
            // selection reads, so it gets the same ring as a mosaic tile.
            if focused {
                focus_ring(ui.painter(), rect);
            }
            // Mini glyph.
            let glyph_c = egui::pos2(rect.center().x, rect.min.y + THUMB_H * 0.36);
            paint_kind_glyph(
                ui.painter(),
                glyph_c,
                THUMB_H * 0.2,
                unit.kind,
                cat.accent(),
            );
            // Status dot.
            if let Some(h) = unit.health {
                ui.painter().circle_filled(
                    rect.right_top() + Vec2::new(-Style::SP_S, Style::SP_S),
                    Style::SP_XS * 0.7,
                    h.ring_color(),
                );
            }
            // The pin marker (O9).
            if pinned {
                paint_pin(
                    ui.painter(),
                    rect.min + Vec2::splat(Style::SP_S),
                    Style::ACCENT_HI,
                );
            }
            // Truncated name.
            let name = truncate(&unit.name, 12);
            ui.painter().text(
                egui::pos2(rect.center().x, rect.max.y - Style::SP_S),
                Align2::CENTER_BOTTOM,
                name,
                FontId::proportional(Style::SMALL),
                Style::TEXT,
            );
        })
        .response;
    let resp = resp.on_hover_text(&unit.name);
    // a11y-05 — the thumbnail's accesskit node (a filmstrip thumb has no mark
    // set, so `marked` is false; the pin marker rides the value).
    install_unit_accessibility(ui.ctx(), resp.id, unit, pinned, false, resp.rect);
    (resp.clicked(), resp.secondary_clicked())
}

/// Truncate a name to `max` chars with an ellipsis, so a long id never blows the
/// thumbnail width.
pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

// ─────────────────── mosaic overview render (EXPLORER-11) ───────────────────

/// The mosaic/filmstrip cluster a unit files under (EXPLORER-16, O9): the
/// **Pinned** front cluster, else its proximity category — the grouping key the
/// cluster headers and filmstrip dividers speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cluster {
    /// The operator's pinned units — always the front cluster (O9).
    Pinned,
    /// A proximity-category run (O1/O8).
    Cat(Category),
}

impl Cluster {
    /// The header / divider label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Pinned => "Pinned",
            Self::Cat(c) => c.label(),
        }
    }

    /// The header / divider accent — the pin cluster wears the highlight accent
    /// (§4 token, like the focus ring), categories keep their O8 identity.
    pub(super) const fn accent(self) -> Color32 {
        match self {
            Self::Pinned => Style::ACCENT_HI,
            Self::Cat(c) => c.accent(),
        }
    }
}

/// A D-pad direction over the mosaic grid (O6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GridDir {
    Left,
    Right,
    Up,
    Down,
}

/// Move the focus index over a `count`-item, `cols`-wide grid one step in `dir`,
/// clamping at every edge (a D-pad press past an edge stays put — never wraps, so
/// couch nav is predictable, O6). Pure — the grid-nav model, unit-tested without a
/// render.
pub(super) fn grid_move(focus: usize, count: usize, cols: usize, dir: GridDir) -> usize {
    if count == 0 {
        return 0;
    }
    let cols = cols.max(1);
    let last = count - 1;
    match dir {
        GridDir::Left => focus.saturating_sub(1),
        GridDir::Right => (focus + 1).min(last),
        // Top row can't rise; else step up a whole row.
        GridDir::Up => focus.checked_sub(cols).unwrap_or(focus),
        GridDir::Down => (focus + cols).min(last),
    }
}

/// The number of mosaic columns that fit in `avail` pixels (always ≥1, even at a
/// nonsense/negative width), so the grid-nav row step and the rendered row width
/// agree — a zero-column grid would render nothing.
pub(super) fn mosaic_columns(avail: f32) -> usize {
    (((avail + MOSAIC_GAP) / (MOSAIC_TILE_W + MOSAIC_GAP)) as usize).max(1)
}

/// Linear interpolate `a`→`b` by `t`.
pub(super) fn flerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolate a rect `from`→`to` by `t` — the shared-element zoom geometry (O3).
pub(super) fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    Rect::from_min_max(
        egui::pos2(
            flerp(from.min.x, to.min.x, t),
            flerp(from.min.y, to.min.y, t),
        ),
        egui::pos2(
            flerp(from.max.x, to.max.x, t),
            flerp(from.max.y, to.max.y, t),
        ),
    )
}

/// An ease-out curve (fast-in, settling) for the zoom reveal — Carbon productive
/// motion without pulling in a bespoke easing framework.
pub(super) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// One health-rollup stat (O2): a filled status dot in `color` + its count, so the
/// green/warn/down palette reads at a glance in the summary strip.
pub(super) fn health_dot(ui: &mut egui::Ui, color: Color32, count: usize) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_XS;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(Style::SP_S), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), Style::SP_XS * 0.9, color);
        ui.label(
            RichText::new(count.to_string())
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
}

/// A mosaic cluster header (O1/O8/O9): the cluster label (Pinned or a category)
/// + its count in the cluster accent — the clustered grid's divider between runs.
pub(super) fn mosaic_cluster_header(ui: &mut egui::Ui, cluster: Cluster, count: usize) {
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        ui.label(
            RichText::new(cluster.label())
                .size(Style::BODY)
                .strong()
                .color(cluster.accent()),
        );
        ui.label(
            RichText::new(count.to_string())
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
    ui.add_space(Style::SP_XS);
}

/// One mosaic hero-tile (EXPLORER-11): a mini status ring + kind glyph, the
/// truncated name, and a type badge, in a category-tinted frame; the keyboard/
/// D-pad-focused tile wears a thick high-contrast focus ring (O11); a pinned
/// tile wears the pin marker (O9); a **marked** tile (EXPLORER-17 multi-select)
/// wears an accent frame + a filled mark square at top-right. Hand-painted so
/// the procedural glyph family (O8) rides inside, echoing the hero at tile
/// scale. Returns its rect (the zoom-in origin), whether it was clicked (the
/// O3 pick), and whether it was right-clicked (the O9 pin toggle).
pub(super) fn mosaic_tile(
    ui: &mut egui::Ui,
    unit: &Unit,
    focused: bool,
    pinned: bool,
    marked: bool,
) -> (Rect, bool, bool) {
    let cat = unit.kind.category();
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(MOSAIC_TILE_W, MOSAIC_TILE_H), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    // The shared raised-card depth (§4): the tile rests at Raised and
    // hover-lifts toward Overlay — cast under the fill, no layout shift.
    raise(painter, rect, ("explorer-tile-raise", &unit.id), hovered);
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    // The frame: the mark accent, a hover accent, or a calm border — then the
    // shared thick focus ring OVER it for the selection (EXPLORER-18, O11 —
    // always legible for D-pad nav; a marked tile stays visibly in the set).
    let frame = if marked {
        Style::ACCENT
    } else if hovered {
        cat.accent()
    } else {
        Style::BORDER
    };
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        Stroke::new(1.0, frame),
        StrokeKind::Inside,
    );
    if focused {
        focus_ring(painter, rect);
    }
    // The EXPLORER-17 mark: a small filled accent square at top-right (the O9
    // pin keeps top-left), so a marked tile reads at a glance in the grid.
    if marked {
        let m = Rect::from_center_size(
            egui::pos2(rect.max.x - Style::SP_S, rect.min.y + Style::SP_S),
            Vec2::splat(Style::SP_S),
        );
        painter.rect_filled(m, Style::RADIUS * 0.3, Style::ACCENT);
    }
    // The mini status ring + kind glyph (echoes the hero, O1/O8). A known health
    // tier tints the ring; an unprobed unit reads as a calm border, never faked.
    let ring_c = egui::pos2(
        rect.center().x,
        rect.min.y + MOSAIC_RING_D * 0.5 + Style::SP_S,
    );
    let ring_r = MOSAIC_RING_D * 0.5;
    let ring_color = unit.health.map_or(Style::BORDER, Health::ring_color);
    painter.circle_stroke(ring_c, ring_r, Stroke::new(RING_STROKE_W, ring_color));
    paint_kind_glyph(painter, ring_c, ring_r * 0.55, unit.kind, cat.accent());
    // The truncated name + the type badge under it.
    painter.text(
        egui::pos2(rect.center().x, rect.max.y - Style::SP_M),
        Align2::CENTER_BOTTOM,
        truncate(&unit.name, 14),
        FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(rect.center().x, rect.max.y - Style::SP_XS),
        Align2::CENTER_BOTTOM,
        unit.kind.label(),
        FontId::proportional(Style::SMALL),
        cat.accent(),
    );
    // The pin marker (O9) in the tile's top-left corner.
    if pinned {
        paint_pin(
            painter,
            rect.min + Vec2::splat(Style::SP_S),
            Style::ACCENT_HI,
        );
    }
    let resp = resp.on_hover_text(if pinned {
        "Right-click to unpin · Ctrl-click / Space marks"
    } else {
        "Right-click to pin · Ctrl-click / Space marks"
    });
    // a11y-05 — the tile's accesskit node (name + kind/reachability/health +
    // pinned/marked), keyed by the cell response id. Pure metadata.
    install_unit_accessibility(ui.ctx(), resp.id, unit, pinned, marked, rect);
    (rect, resp.clicked(), resp.secondary_clicked())
}

/// A tiny procedural pushpin marker (O9): a filled head + a 45° stem — painter
/// primitives in the given §4 accent, like the kind glyphs.
pub(super) fn paint_pin(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let r = Style::SP_XS;
    painter.circle_filled(
        egui::pos2(center.x + r * 0.35, center.y - r * 0.35),
        r * 0.6,
        color,
    );
    painter.line_segment(
        [
            egui::pos2(center.x + r * 0.1, center.y - r * 0.1),
            egui::pos2(center.x - r * 0.8, center.y + r * 0.8),
        ],
        Stroke::new(GLYPH_STROKE_W * 0.75, color),
    );
}

// ─────────────────── IPAM table render (EXPLORER-10) ───────────────────

/// The flexible occupant-name column width: the row less the fixed address + type
/// columns and the leading indent, floored so a narrow surface still shows a name.
pub(super) fn ipam_name_col_w(avail: f32) -> f32 {
    (avail - Style::SP_M - IPAM_ADDR_COL - IPAM_TYPE_COL).max(Style::SP_XL * 2.0)
}

/// A rough char budget for a name column of `width` at the body face — keeps a long
/// name inside its cell rather than overrunning the type column.
pub(super) fn ipam_name_budget(width: f32) -> usize {
    ((width / (Style::BODY * 0.6)) as usize).max(6)
}

/// A dim small-face `RichText` for a table caption / column header.
pub(super) fn ipam_dim(text: &str) -> RichText {
    RichText::new(text)
        .size(Style::SMALL)
        .color(Style::TEXT_DIM)
}

/// A fixed-width table cell holding one left-aligned label (keeps the columns
/// aligned across every prefix's rows).
pub(super) fn ipam_cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, IPAM_ROW_H),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(text);
        },
    );
}

/// The prefix header band (design E7): the CIDR + category badge + discovered
/// tenant-net label on the left; the capacity meter, free/used tally, and gateway
/// on the right. A subtle `SURFACE_HI` band with a category-accent tab.
pub(super) fn ipam_prefix_header(ui: &mut egui::Ui, p: &IpamPrefix) {
    let accent = p.category.accent();
    // Reserve the band + accent-tab slots so they paint BEHIND the row content.
    let band = ui.painter().add(egui::Shape::Noop);
    let tab = ui.painter().add(egui::Shape::Noop);
    let rect = ui
        .horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(IPAM_ROW_H);
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(p.cidr())
                    .monospace()
                    .strong()
                    .color(Style::TEXT),
            );
            ui.label(
                RichText::new(p.category.label())
                    .size(Style::SMALL)
                    .color(accent)
                    .background_color(Style::SURFACE),
            );
            if let Some(label) = &p.label {
                ui.label(ipam_dim(&format!("· {label}")));
            }
            // The right cluster: gateway · free/used · capacity meter.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                let gw = p.gateway();
                let gw_txt = p
                    .occupants
                    .iter()
                    .find(|o| o.addr == gw)
                    .map_or_else(|| format!("gw {gw}"), |o| format!("gw {gw} · {}", o.name));
                ui.label(
                    RichText::new(gw_txt)
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_M);
                ui.label(ipam_dim(&format!("{} used · {} free", p.used(), p.free())));
                ui.add_space(Style::SP_S);
                used_free_bar(ui, p.used(), accent);
            });
        })
        .response
        .rect;
    ui.painter().set(
        band,
        egui::Shape::rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE_HI),
    );
    let tab_rect = Rect::from_min_max(rect.min, egui::pos2(rect.min.x + Style::SP_XS, rect.max.y));
    ui.painter()
        .set(tab, egui::Shape::rect_filled(tab_rect, 0.0, accent));
}

/// The slim column-header row under a prefix band (Address · Occupant · Type),
/// aligned to the address rows' fixed columns.
pub(super) fn ipam_column_header(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let name_w = ipam_name_col_w(ui.available_width());
        ui.add_space(Style::SP_M);
        ipam_cell(ui, IPAM_ADDR_COL, ipam_dim("Address"));
        ipam_cell(ui, name_w, ipam_dim("Occupant"));
        ipam_cell(ui, IPAM_TYPE_COL, ipam_dim("Type"));
    });
}

/// One occupied-address row: the address (mono; the gateway host accent-tinted),
/// the occupant name (a link-toned jump affordance), and its type badge. Zebra
/// banded, hover-highlit, and clickable — a click jumps the hero focus to the
/// occupant. Returns whether it was clicked.
pub(super) fn ipam_address_row(
    ui: &mut egui::Ui,
    occ: &IpamOccupant,
    gw: Ipv4Addr,
    zebra: bool,
) -> bool {
    let accent = occ.kind.category().accent();
    let is_gw = occ.addr == gw;
    // Reserve the zebra band slot so it paints BEHIND the row content.
    let band = ui.painter().add(egui::Shape::Noop);
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.set_min_width(ui.available_width());
            let name_w = ipam_name_col_w(ui.available_width());
            ui.add_space(Style::SP_M);
            ipam_cell(
                ui,
                IPAM_ADDR_COL,
                RichText::new(occ.addr.to_string())
                    .monospace()
                    .color(if is_gw { accent } else { Style::TEXT }),
            );
            ipam_cell(
                ui,
                name_w,
                RichText::new(truncate(&occ.name, ipam_name_budget(name_w)))
                    .size(Style::BODY)
                    .color(Style::ACCENT_HI),
            );
            ipam_cell(
                ui,
                IPAM_TYPE_COL,
                RichText::new(occ.kind.label())
                    .size(Style::SMALL)
                    .color(accent),
            );
        })
        .response
        .interact(Sense::click());
    let fill = if resp.hovered() {
        Style::SURFACE_HI
    } else if zebra {
        Style::SURFACE
    } else {
        Style::BG
    };
    ui.painter().set(
        band,
        egui::Shape::rect_filled(resp.rect, Style::RADIUS * 0.5, fill),
    );
    let resp = resp.on_hover_text(format!("Jump to {}", occ.name));
    // a11y-05 — the IPAM occupant row's accesskit node: the occupant name +
    // its kind/address (+ gateway marker for the conventional .1).
    install_cell_accessibility(
        ui.ctx(),
        resp.id,
        occ.name.clone(),
        occupant_a11y_value(occ, is_gw),
        resp.rect,
    );
    resp.clicked()
}

/// The prefix capacity meter: a thin bar with the used fraction of the /24 filled
/// in the category accent over a surface track (the honest used/free ratio).
pub(super) fn used_free_bar(ui: &mut egui::Ui, used: usize, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(IPAM_BAR_W, Style::SP_S), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE);
    let frac = (used as f32 / IPAM_USABLE_PER_24 as f32).clamp(0.0, 1.0);
    if frac > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
        painter.rect_filled(fill, Style::RADIUS * 0.5, accent);
    }
    painter.rect_stroke(
        rect,
        Style::RADIUS * 0.5,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
}

/// The hero card body (#9/#10/#11/#12): the status ring + type glyph, the
/// name/type/reachability headline, and rich telemetry when reachable else a
/// dimmed-minimal card with explicit unknowns. `discovering` renders the #23
/// self card's "Discovering units…" line; `history` carries the focused unit's
/// rolling sparkline samples (EXPLORER-4, `None` for the placeholder/dimmed path).
pub(super) fn hero_card(
    ui: &mut egui::Ui,
    unit: &Unit,
    discovering: bool,
    history: Option<&UnitHistory>,
) {
    let cat = unit.kind.category();
    let rich = hero_is_rich(unit);
    ui.add_space(Style::SP_L);

    // The status ring + type glyph (#9).
    let side =
        (ui.available_width().min(ui.available_height()) * RING_FRACTION).clamp(RING_MIN, RING_MAX);
    let (ring_rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let center = ring_rect.center();
    let radius = side * 0.5 - RING_STROKE_W;
    let time = ui.input(|i| i.time);
    let spinning = paint_status_ring(
        ui.painter(),
        center,
        radius,
        unit.health,
        cat.accent(),
        time,
    );
    paint_kind_glyph(ui.painter(), center, radius * 0.5, unit.kind, cat.accent());
    if spinning {
        ui.ctx().request_repaint();
    }

    ui.add_space(Style::SP_M);

    // Name + type badge + reachability (#10).
    ui.label(
        RichText::new(&unit.name)
            .size(HERO_TITLE_FS)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        // Centre the badge row within the top-down-centre layout.
        ui.add_space(ui.available_width() * 0.5 - Style::SP_XL * 2.0);
        ui.label(
            RichText::new(unit.kind.label())
                .size(Style::SMALL)
                .color(cat.accent())
                .background_color(Style::SURFACE_HI),
        );
        ui.label(
            RichText::new(reachability_line(
                &unit.reachability,
                unit.address.as_deref(),
            ))
            .size(Style::BODY)
            .color(Style::TEXT_DIM),
        );
    });

    ui.add_space(Style::SP_M);

    if discovering {
        muted_note(ui, "Discovering units… others stream in as they're found.");
        return;
    }

    if rich {
        hero_telemetry(ui, unit, history);
    } else {
        // Dimmed-minimal card (#12) — only what's known, no faked fields (§7).
        ui.scope(|ui| {
            ui.set_opacity(DIMMED_OPACITY);
            let note = match unit.reachability {
                Reachability::OnLan => "Outside the mesh — limited detail until adopted.",
                _ => "Not reachable — showing only what's known.",
            };
            muted_note(ui, note);
        });
    }

    // First/last-seen footer (E10) — real presence, honest for a fresh unit.
    if unit.last_seen_ms > 0 {
        let now = now_ms();
        let ago = fmt_seen_ago(now.saturating_sub(unit.last_seen_ms));
        let tracked = fmt_duration(unit.last_seen_ms.saturating_sub(unit.first_seen_ms) / 1_000);
        ui.add_space(Style::SP_M);
        ui.label(
            RichText::new(format!("Last seen {ago} · tracked {tracked}"))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
}

/// Whether the unit is "reachable-rich" (#11): a live mesh peer or a cloud
/// instance we can read, vs an outside/unreachable unit that gets the dimmed card.
pub(super) const fn hero_is_rich(unit: &Unit) -> bool {
    match unit.kind {
        UnitKind::Peer => {
            matches!(unit.reachability, Reachability::InMesh)
                && matches!(
                    unit.health,
                    Some(Health::Healthy | Health::Degraded | Health::Critical)
                )
        }
        UnitKind::Instance => true,
        _ => false,
    }
}

/// The rich telemetry region (#11, EXPLORER-4): the health pill, a peer's mesh
/// facts (role/leader/version), and the **metric grid** — load / mem / net /
/// uptime, load and mem drawing a real sparkline from `history` — or an honest
/// "Live telemetry not yet reported" line when a readable unit has nothing to
/// show yet (§7).
pub(super) fn hero_telemetry(ui: &mut egui::Ui, unit: &Unit, history: Option<&UnitHistory>) {
    let accent = unit.kind.category().accent();
    if let Some(health) = unit.health {
        ui.label(
            RichText::new(health_label(health))
                .size(Style::BODY)
                .color(health.ring_color()),
        );
    }
    if let Some(mesh) = &unit.mesh {
        let mut facts = Vec::new();
        if let Some(role) = &mesh.role {
            facts.push(role.clone());
        }
        if mesh.leader {
            facts.push("leader".to_string());
        }
        if let Some(v) = &mesh.mde_version {
            facts.push(format!("mde {v}"));
        }
        if !facts.is_empty() {
            ui.label(
                RichText::new(facts.join(" · "))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    }

    // The load/mem sparklines draw only from real accumulated samples (§7).
    let load_series = history.map(|h| &h.load1).filter(|s| !s.is_empty());
    let mem_series = history.map(|h| &h.mem_used_pct).filter(|s| !s.is_empty());
    let telemetry = unit.telemetry.clone().unwrap_or_default();
    // Show the grid once there's *anything* real to show — a scalar this tick or
    // an accumulated trend; otherwise the honest "nothing yet" line, not a wall
    // of empty cells.
    if telemetry.any() || load_series.is_some() || mem_series.is_some() {
        ui.add_space(Style::SP_S);
        metric_grid(ui, &telemetry, load_series, mem_series, accent);
    } else {
        muted_note(ui, "Live telemetry not yet reported.");
    }
}

/// The centred load · mem · net · uptime metric grid (EXPLORER-4). A fixed-width
/// row so the surrounding top-down-centre layout centres it cleanly. Each metric
/// is honest field-by-field: a readable value + sparkline where a source exists,
/// a dimmed "no source" cell where none does (net), never a fabricated trend.
pub(super) fn metric_grid(
    ui: &mut egui::Ui,
    t: &Telemetry,
    load_series: Option<&VecDeque<f32>>,
    mem_series: Option<&VecDeque<f32>>,
    accent: Color32,
) {
    let row_w = SPARK_W * 4.0 + Style::SP_L * 3.0;
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, METRIC_CELL_H),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_L;
            metric_cell(
                ui,
                "load",
                t.load1.map(|v| format!("{v:.2}")),
                load_series,
                LOAD_REF_CEIL,
                accent,
            );
            metric_cell(
                ui,
                "mem",
                t.mem_used_pct.map(|v| format!("{v:.0}%")),
                mem_series,
                MEM_FULL_SCALE,
                accent,
            );
            // Net has no live source on today's mirror — an honest dimmed cell,
            // not a faked throughput curve (§7). It lights up when the aggregator
            // begins reporting a rate.
            metric_cell(ui, "net", None, None, 0.0, accent);
            // Uptime is a scalar counter, not a trend — show the value with a
            // neutral baseline rather than a meaningless ramp.
            metric_cell(
                ui,
                "uptime",
                t.uptime_s.map(fmt_duration),
                None,
                0.0,
                accent,
            );
        },
    );
}

/// One metric cell: the current value (or a dimmed "—" when unreadable), a
/// sparkline of the real observed `series` when it has ≥2 points, and a caption.
/// The placeholder is honest per case: "collecting…" for a readable metric still
/// filling its trend, a neutral baseline for a scalar-only metric, "no source"
/// where nothing is reported at all (§7).
pub(super) fn metric_cell(
    ui: &mut egui::Ui,
    caption: &str,
    value: Option<String>,
    series: Option<&VecDeque<f32>>,
    full_scale: f32,
    color: Color32,
) {
    ui.allocate_ui_with_layout(
        Vec2::new(SPARK_W, METRIC_CELL_H),
        Layout::top_down(Align::Center),
        |ui| {
            ui.set_min_width(SPARK_W);
            let has_value = value.is_some();
            match value {
                Some(v) => ui.label(
                    RichText::new(v)
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT),
                ),
                None => ui.label(
                    RichText::new("—")
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT_DIM),
                ),
            };
            match (series, has_value) {
                (Some(s), _) if s.len() >= 2 => sparkline(ui, s, full_scale, color),
                // A readable series metric that hasn't filled two points yet.
                (Some(_), _) => spark_note(ui, "collecting…"),
                // A scalar-only metric (uptime): a neutral baseline, no fake trend.
                (None, true) => spark_baseline(ui),
                // No live source at all (net): honestly dimmed unknown.
                (None, false) => spark_note(ui, "no source"),
            }
            ui.label(
                RichText::new(caption)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        },
    );
}

/// Draw a sparkline of `samples` (oldest → newest) scaled to `[0, full_scale]`,
/// the axis expanding to fit any real peak above the reference so a spike is
/// never clipped. Newest reading dotted. Real observed points only — the caller
/// guarantees ≥2 (§7).
pub(super) fn sparkline(
    ui: &mut egui::Ui,
    samples: &VecDeque<f32>,
    full_scale: f32,
    color: Color32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE);
    let n = samples.len();
    if n < 2 {
        return;
    }
    // Scale to the metric's reference, but never below a real peak (no clipping).
    let peak = samples
        .iter()
        .copied()
        .fold(full_scale, f32::max)
        .max(f32::EPSILON);
    let pad = SPARK_STROKE_W;
    let plot_h = rect.height() - pad * 2.0;
    let x_at = |i: usize| rect.min.x + rect.width() * (i as f32 / (n - 1) as f32);
    let y_at = |v: f32| rect.max.y - pad - plot_h * (v / peak).clamp(0.0, 1.0);
    let stroke = Stroke::new(SPARK_STROKE_W, color);
    let pts: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, &v)| egui::pos2(x_at(i), y_at(v)))
        .collect();
    for seg in pts.windows(2) {
        painter.line_segment([seg[0], seg[1]], stroke);
    }
    // Emphasise the newest reading with a dot.
    if let Some(&last) = pts.last() {
        painter.circle_filled(last, SPARK_STROKE_W * 1.5, color);
    }
}

/// A dimmed placeholder occupying the sparkline's footprint (keeps the grid rows
/// aligned) with an honest short caption — "collecting…" / "no source" (§7).
pub(super) fn spark_note(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// A neutral baseline in the sparkline footprint for a scalar-only metric (its
/// value is real, but there is no series to trend — so no fabricated ramp, §7).
pub(super) fn spark_baseline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.center().y),
            egui::pos2(rect.max.x, rect.center().y),
        ],
        Stroke::new(1.0, Style::BORDER),
    );
}

/// The human label for a health tier.
pub(super) const fn health_label(health: Health) -> &'static str {
    match health {
        Health::Healthy => "Healthy",
        Health::Degraded => "Degraded",
        Health::Critical => "Critical",
        Health::Unreachable => "Unreachable",
        Health::Unknown => "Status unknown",
    }
}

// ── accesskit (a11y-05 / shell-ux-6) ─────────────────────────────────────────
//
// The fleet Explorer's pickable items — the mosaic tiles, the filmstrip
// thumbnails, the `/` search-hit rows, the edge jump-chips, and the IPAM
// occupant rows — are every one a hand-rolled `allocate_exact_size(click)` /
// `scope_builder(...).sense(click)` / `.interact(click)` cell painted with raw
// `Painter` calls. egui auto-generates accesskit nodes only for real widgets
// via `Response::widget_info`, never for these raw cells (the same gap dock.rs
// / console.rs closed for their own under WIN7-5/WIN7-7), so a screen reader
// walking the mesh heard nothing. This section gives each pickable item its own
// `Role::Button` node keyed by the cell's response id (so egui merges it onto
// the cell), with the unit/occupant name as the accessible label and its
// kind / reachability / health (+ pinned / marked / gateway) reading as the
// value — the established per-module `install_*_accessibility` idiom
// (role + label + value + bounds + Click), state carried in the value string
// exactly as dock.rs/console.rs already do.

/// Convert an egui rect to an accesskit one (the `console.rs`/`dock.rs` helper,
/// restated module-locally — the established per-module-copy idiom).
pub(super) fn accesskit_rect(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The accessible **name** of a unit cell — its big display name (the same
/// string the tile / thumbnail / search row paints).
pub(super) fn unit_a11y_label(unit: &Unit) -> String {
    unit.name.clone()
}

/// The accessible **state/value** of a unit cell — the kind badge, the
/// reachability/address line ([`reachability_line`]), the health tier when a
/// source reports one ([`health_label`]), and the pinned/marked markers the
/// cell paints as corner glyphs. Mirrors what a sighted operator reads off the
/// tile so the two can't drift.
pub(super) fn unit_a11y_state(unit: &Unit, pinned: bool, marked: bool) -> String {
    let mut parts = vec![
        unit.kind.label().to_owned(),
        reachability_line(&unit.reachability, unit.address.as_deref()),
    ];
    if let Some(h) = unit.health {
        parts.push(health_label(h).to_owned());
    }
    if pinned {
        parts.push("pinned".to_owned());
    }
    if marked {
        parts.push("marked".to_owned());
    }
    parts.join(" \u{00B7} ")
}

/// The accessible **value** of an IPAM occupant row — its kind, its address,
/// and the gateway marker for the conventional `.1` (the same facts the row
/// paints across its columns).
pub(super) fn occupant_a11y_value(occ: &IpamOccupant, is_gateway: bool) -> String {
    let mut value = format!("{} \u{00B7} {}", occ.kind.label(), occ.addr);
    if is_gateway {
        value.push_str(" \u{00B7} gateway");
    }
    value
}

/// Install one raw-painted cell's accesskit `Button` node, keyed by the cell's
/// own response id so egui merges it onto the cell (the dock.rs id-keyed merge).
/// Shared by every pickable item this module paints so the role/label/value/
/// bounds/action shape can never drift between the tiles, thumbnails, rows, and
/// chips.
pub(super) fn install_cell_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    label: impl Into<String>,
    value: impl Into<String>,
    rect: Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.into());
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

/// Install a unit-bearing cell's accesskit node (tile / thumbnail / search row).
pub(super) fn install_unit_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    unit: &Unit,
    pinned: bool,
    marked: bool,
    rect: Rect,
) {
    install_cell_accessibility(
        ctx,
        id,
        unit_a11y_label(unit),
        unit_a11y_state(unit, pinned, marked),
        rect,
    );
}
