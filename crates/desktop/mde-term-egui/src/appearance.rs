//! Appearance knobs (TERM-11) — the terminal's content palette, font size and
//! cursor style, plus the simple picker that edits them.
//!
//! Design lock Q13 keeps this deliberately small: **no full profile system**,
//! just the three knobs a user actually reaches for — a colour scheme (the
//! Construct default or a bundled classic, [`crate::presets`]), the content font
//! size, and the cursor style (block / bar / underline, steady or blink). An
//! [`Appearance`] is that bundle; the surface holds one and pushes it into every
//! live pane each frame, so a change in the picker reaches all shells at once.
//!
//! §4: the picker chrome is pure `Style` tokens. The colour **swatches** it
//! previews are the content palette's own data ([`crate::presets`]) — the same
//! carve-out the grid renders under, shown so a scheme is pickable by eye.

use mde_egui::egui::{
    self, Align, Align2, Area, Key, Layout, Order, Pos2, Rect, RichText, Sense, StrokeKind,
    UiBuilder, Vec2,
};
use mde_egui::Style;

use crate::palette::Palette;
use crate::presets::Preset;

/// The smallest / largest content font size the picker allows, in points, and
/// the step of one nudge. A terminal wants a legible floor and a sane ceiling —
/// this is the knob's range, not a profile system.
const FONT_MIN: f32 = 8.0;
const FONT_MAX: f32 = 40.0;
const FONT_STEP: f32 = 1.0;

/// The picker panel's fixed width.
const PANEL_WIDTH: f32 = 320.0;
/// A preset row's colour-swatch strip size.
const SWATCH_W: f32 = 120.0;
const SWATCH_H: f32 = 14.0;

/// How the cursor cell is shaped (design lock Q13's cursor-style knob). Steady
/// vs blink is the separate [`Appearance::cursor_blink`] flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorShape {
    /// A full-cell block — the classic terminal cursor.
    #[default]
    Block,
    /// A thin vertical bar at the cell's left edge (the "I-beam" insert cursor).
    Bar,
    /// A thin underline along the cell's bottom edge.
    Underline,
}

impl CursorShape {
    /// The three shapes, in the picker's order.
    pub const ALL: [Self; 3] = [Self::Block, Self::Bar, Self::Underline];

    /// The picker label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::Bar => "Bar",
            Self::Underline => "Underline",
        }
    }

    /// The sub-rect a **focused** cursor of this shape fills within its cell
    /// `block`: the whole cell for [`Block`](Self::Block), a left-edge sliver for
    /// [`Bar`](Self::Bar), a bottom-edge sliver for [`Underline`](Self::Underline).
    /// A pure fold so the widget's cursor paint is unit-tested without a GPU.
    #[must_use]
    pub fn rect(self, block: Rect) -> Rect {
        // A sliver is ~1/6 of the cell, at least one pixel — visible at every
        // font size without swallowing the glyph.
        let bar_w = (block.width() / 6.0).max(1.0);
        let under_h = (block.height() / 6.0).max(1.0);
        match self {
            Self::Block => block,
            Self::Bar => Rect::from_min_max(block.min, Pos2::new(block.min.x + bar_w, block.max.y)),
            Self::Underline => {
                Rect::from_min_max(Pos2::new(block.min.x, block.max.y - under_h), block.max)
            }
        }
    }
}

/// The three appearance knobs (design lock Q13). Small and `Copy` so the surface
/// can hand a snapshot to every pane each frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Appearance {
    /// The active content colour scheme.
    pub palette: Palette,
    /// The content font size in points.
    pub font_size: f32,
    /// The cursor shape.
    pub cursor_shape: CursorShape,
    /// Whether the focused cursor blinks (steady when `false`).
    pub cursor_blink: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            palette: Palette::from_tokens(),
            font_size: Style::BODY,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
        }
    }
}

/// The appearance picker (TERM-11): a small overlay panel that edits the
/// surface's [`Appearance`] in place.
///
/// Scheme presets with colour swatches, a font-size stepper, and the
/// cursor-style / blink controls. Toggled from the tab bar or `Ctrl+Shift+P` —
/// purely a UI shell over the [`Appearance`] it is handed.
#[derive(Default)]
pub struct AppearancePicker {
    open: bool,
}

impl AppearancePicker {
    /// A fresh, closed picker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the picker is shown.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the picker.
    pub const fn open(&mut self) {
        self.open = true;
    }

    /// Close the picker.
    pub const fn close(&mut self) {
        self.open = false;
    }

    /// Toggle the picker open/closed (the tab-bar button + `Ctrl+Shift+P`).
    pub const fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Render the overlay (a no-op while closed), editing `appearance` in place.
    /// Escape or the Close button dismisses it.
    pub fn show(&mut self, ctx: &egui::Context, appearance: &mut Appearance) {
        if !self.open {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.close();
            return;
        }

        let mut close = false;
        Area::new(egui::Id::new("term-appearance-picker"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, Style::SP_XL))
            .show(ctx, |ui| {
                // Reserve the shadow + plate + border behind the content, size
                // them to the laid content, exactly as the remote picker's card
                // does. §4. The first slot is the shared Overlay-elevation shadow
                // so this floating popover reads as lifted off the grid.
                let margin = Style::SP_M;
                let shadow = ui.painter().add(egui::Shape::Noop);
                let bg = ui.painter().add(egui::Shape::Noop);
                let border = ui.painter().add(egui::Shape::Noop);

                let start = ui.min_rect().min + Vec2::splat(margin);
                let mut content = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(
                            start,
                            Vec2::new(PANEL_WIDTH, ui.available_height()),
                        ))
                        .layout(Layout::top_down(Align::Min)),
                );
                content.set_width(PANEL_WIDTH);
                close = Self::panel(&mut content, appearance);

                let plate = content.min_rect().expand(margin);
                ui.painter()
                    .set(shadow, crate::overlay::overlay_shadow(plate));
                ui.painter().set(
                    bg,
                    egui::Shape::rect_filled(plate, Style::RADIUS, Style::SURFACE),
                );
                ui.painter().set(
                    border,
                    egui::Shape::rect_stroke(
                        plate,
                        Style::RADIUS,
                        Style::hairline(),
                        StrokeKind::Inside,
                    ),
                );
                ui.allocate_rect(plate, Sense::hover());
            });
        if close {
            self.close();
        }
    }

    /// The panel body. Returns `true` when the user asked to close it.
    fn panel(ui: &mut egui::Ui, appearance: &mut Appearance) -> bool {
        ui.label(RichText::new("Appearance").color(Style::TEXT).strong());
        ui.add_space(Style::SP_XS);

        // ── Colour scheme ────────────────────────────────────────────────────
        ui.label(RichText::new("Scheme").color(Style::TEXT_DIM).small());
        ui.add_space(Style::SP_XS);
        let active = Preset::matching(&appearance.palette);
        for preset in Preset::ALL {
            if Self::preset_row(ui, preset, active == Some(preset)) {
                appearance.palette = preset.palette();
            }
        }

        ui.add_space(Style::SP_S);
        Self::hairline(ui);
        ui.add_space(Style::SP_S);

        // ── Font size ────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label(RichText::new("Font size").color(Style::TEXT_DIM).small());
            if ui
                .button(RichText::new("\u{2212}").color(Style::TEXT))
                .clicked()
            {
                appearance.font_size = (appearance.font_size - FONT_STEP).max(FONT_MIN);
            }
            ui.label(
                RichText::new(format!("{:.0} pt", appearance.font_size))
                    .color(Style::TEXT)
                    .monospace(),
            );
            if ui.button(RichText::new("+").color(Style::TEXT)).clicked() {
                appearance.font_size = (appearance.font_size + FONT_STEP).min(FONT_MAX);
            }
        });

        ui.add_space(Style::SP_S);

        // ── Cursor ───────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label(RichText::new("Cursor").color(Style::TEXT_DIM).small());
            for shape in CursorShape::ALL {
                ui.selectable_value(&mut appearance.cursor_shape, shape, shape.label());
            }
        });
        ui.add_space(Style::SP_XS);
        ui.checkbox(&mut appearance.cursor_blink, "Blink");

        ui.add_space(Style::SP_S);
        ui.button("Close").clicked()
    }

    /// One scheme row: its 16-colour swatch strip + name, the whole row a single
    /// click target that lights the accent when active or hovered. Returns
    /// whether it was picked. The swatches paint the preset's own palette data
    /// (the content carve-out); every other pixel is a `Style` token (§4).
    #[allow(clippy::cast_precision_loss)] // swatch index/count → f32 pixel offsets.
    fn preset_row(ui: &mut egui::Ui, preset: Preset, active: bool) -> bool {
        let palette = preset.palette();
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 2.0f32.mul_add(Style::SP_XS, SWATCH_H)),
            Sense::click(),
        );
        let painter = ui.painter();
        let hot = active || resp.hovered();
        if hot {
            painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
        }
        // The swatch strip: 16 equal cells of the scheme's ANSI colours.
        let strip = Rect::from_min_size(
            Pos2::new(rect.min.x + Style::SP_XS, rect.center().y - SWATCH_H / 2.0),
            Vec2::new(SWATCH_W, SWATCH_H),
        );
        let cell_w = strip.width() / palette.ansi.len() as f32;
        for (i, &color) in palette.ansi.iter().enumerate() {
            let x = (i as f32).mul_add(cell_w, strip.min.x);
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(cell_w, strip.height())),
                0.0,
                color,
            );
        }
        painter.rect_stroke(strip, 0.0, Style::hairline(), StrokeKind::Inside);
        // The name, accent when active.
        painter.text(
            Pos2::new(strip.max.x + Style::SP_S, rect.center().y),
            Align2::LEFT_CENTER,
            preset.label(),
            egui::FontId::proportional(Style::BODY),
            if hot { Style::ACCENT } else { Style::TEXT },
        );
        resp.clicked()
    }

    /// A one-pixel hairline separator in the border token.
    fn hairline(ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, Style::BORDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // the default font size is exactly the token.
    fn the_default_appearance_is_the_platform_look() {
        let a = Appearance::default();
        assert_eq!(a.palette, Palette::from_tokens());
        assert_eq!(a.font_size, Style::BODY);
        assert_eq!(a.cursor_shape, CursorShape::Block);
        assert!(a.cursor_blink);
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact edges shared by construction.
    fn cursor_shapes_carve_distinct_rects_from_a_cell() {
        let block = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(12.0, 24.0));
        // Block fills the whole cell.
        assert_eq!(CursorShape::Block.rect(block), block);
        // Bar is a thin left strip: same left/top/bottom, a narrower right edge.
        let bar = CursorShape::Bar.rect(block);
        assert_eq!(bar.min, block.min);
        assert_eq!(bar.max.y, block.max.y);
        assert!(bar.width() < block.width(), "bar is thinner than the cell");
        assert!(bar.width() >= 1.0, "bar stays at least a pixel");
        // Underline is a thin bottom strip: same bottom, a shallower top edge.
        let under = CursorShape::Underline.rect(block);
        assert_eq!(under.max, block.max);
        assert!(under.height() < block.height(), "underline is a sliver");
        assert!(under.min.y > block.min.y, "underline hugs the bottom");
        // The three shapes are genuinely different geometry.
        assert_ne!(bar, block);
        assert_ne!(under, block);
        assert_ne!(bar, under);
    }

    #[test]
    fn a_fresh_picker_is_closed_and_toggles() {
        let mut picker = AppearancePicker::new();
        assert!(!picker.is_open());
        picker.toggle();
        assert!(picker.is_open());
        picker.toggle();
        assert!(!picker.is_open());
    }
}
