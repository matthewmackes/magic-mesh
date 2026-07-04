//! EDTB-3 — the **Word-97 Formatting toolbar** + the **Insert Table grid-picker**
//! (design: `docs/design/editor-toolbar.md` locks #1/#3/#8).
//!
//! The second toolbar row, mounted below the Standard strip (Word's two-row
//! chrome). Every control drives the landed [`md_actions`](crate::md_actions)
//! engine (EDTB-2) for real on the live buffer — no dead buttons (§7). In
//! Word-97 order:
//!
//! * **Style** dropdown → [`set_heading`](crate::md_actions::set_heading): Normal
//!   (body text) or Heading 1-6. Its selected text reads back the primary
//!   caret's current level ([`MenuContext::heading_level`]).
//! * **B / I / U / S** → [`toggle_wrap`](crate::md_actions::toggle_wrap) with
//!   `**` / `*` / `<u>` / `~~` (underline is honest inline HTML — markdown has
//!   none, lock #1).
//! * **Bullet / Numbered list** →
//!   [`toggle_line_prefix`](crate::md_actions::toggle_line_prefix).
//! * **Decrease / Increase Indent** →
//!   [`shift_indent`](crate::md_actions::shift_indent) by ∓1 / ±1 two-space
//!   levels.
//!
//! Each control emits the **same** [`MenuAction`] its Format-menu twin does — one
//! dispatch seam through `EditorSurface::run_action`, zero duplication (§6). The
//! surface applies each op as ONE operator undo step (the widget's `apply_md`
//! records the engine's undo-group count exactly like a fan-out edit). Controls
//! grey out with no open document (the Word grey-out); glyphs are styled text /
//! line-art code points — no new SVG assets (lock #3; the retiring iced-era
//! `IconId` set is not a dependency of this egui surface). Word-style hover
//! tooltips name each control.
//!
//! The **grid-picker** ([`table_grid`]) is Word's drag-grid: hovering highlights
//! the top-left rows×cols block and labels it "C × R Table"; a click commits the
//! size, which [`insert_table`](crate::md_actions::insert_table) drops as a
//! markdown skeleton at the caret.

// The grid geometry converts small cell indices (`usize`) to pixel offsets
// (`f32`) and the hover pixel back to a cell index; every value is bounded by
// the fixed picker grid, so the precision/truncation/sign lints are noise here.
// `suboptimal_flops`: `origin + i * step` reads far clearer than the `mul_add`
// rewrite and the precision/throughput gain is irrelevant for a handful of cell
// positions (the same rationale + repo precedent as `widget.rs`).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]

use mde_egui::egui::{self, pos2, vec2, Rect, RichText, Sense, Stroke, Ui};
use mde_egui::Style;

use crate::menu_bar::{ListStyle, MenuAction, MenuContext, WrapMarker, OVERFLOW_GLYPH};

/// The Style dropdown's labels, indexed by heading level (0 = Normal body text).
pub const STYLE_LABELS: [&str; 7] = [
    "Normal",
    "Heading 1",
    "Heading 2",
    "Heading 3",
    "Heading 4",
    "Heading 5",
    "Heading 6",
];

/// The grid-picker's maximum rows the hover grid offers (Word grows on demand;
/// a fixed generous cap here — the skeleton is editable after insert anyway).
pub const PICKER_ROWS: usize = 8;
/// The grid-picker's maximum columns.
pub const PICKER_COLS: usize = 10;

/// Render one Formatting-strip command button (styled text face + Word tooltip),
/// greyed when `enabled` is false. Returns whether it was clicked this frame.
fn tool(ui: &mut Ui, face: RichText, tip: &'static str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(face))
        .on_hover_text(tip)
        .on_disabled_hover_text(tip)
        .clicked()
}

/// Render the Formatting toolbar and return the action the operator picked this
/// frame, if any. One horizontal strip in Word-97 order; every control greys out
/// with no open document (`cx.has_doc`), exactly like its Format-menu twin.
///
/// When `compact` (EDTB-4 — the panel is narrow), the width-heavy paragraph
/// **Style** dropdown folds into a trailing `»` overflow instead of leading the
/// strip inline, so the narrow icon controls (B/I/U/S, lists, indents) keep the
/// line while every command stays reachable (§7 — relocated, never lost). At
/// full width the Style dropdown renders in line.
pub fn show(ui: &mut Ui, cx: &MenuContext, compact: bool) -> Option<MenuAction> {
    let mut action = None;
    let enabled = cx.has_doc;
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);

        // The paragraph Style dropdown is the one width-heavy control (its text
        // label). Inline at full width; folded into the trailing `»` overflow in
        // compact so the narrow icon controls keep the line.
        if !compact {
            if let Some(picked) = style_dropdown(ui, cx, enabled) {
                action = Some(picked);
            }
            ui.separator();
        }

        // B / I / U / S — the character wraps, each in its own styled face.
        if tool(
            ui,
            RichText::new("B").size(Style::BODY).strong(),
            "Bold",
            enabled,
        ) {
            action = Some(MenuAction::Wrap(WrapMarker::Bold));
        }
        if tool(
            ui,
            RichText::new("I").size(Style::BODY).italics(),
            "Italic",
            enabled,
        ) {
            action = Some(MenuAction::Wrap(WrapMarker::Italic));
        }
        if tool(
            ui,
            RichText::new("U").size(Style::BODY).underline(),
            "Underline",
            enabled,
        ) {
            action = Some(MenuAction::Wrap(WrapMarker::Underline));
        }
        if tool(
            ui,
            RichText::new("S").size(Style::BODY).strikethrough(),
            "Strikethrough",
            enabled,
        ) {
            action = Some(MenuAction::Wrap(WrapMarker::Strike));
        }

        ui.separator();

        // Bullet / Numbered list toggles.
        if tool(
            ui,
            RichText::new("\u{2022}").size(Style::BODY),
            "Bullets",
            enabled,
        ) {
            action = Some(MenuAction::List(ListStyle::Bullet));
        }
        if tool(
            ui,
            RichText::new("1.").size(Style::SMALL),
            "Numbering",
            enabled,
        ) {
            action = Some(MenuAction::List(ListStyle::Numbered));
        }

        ui.separator();

        // Decrease / Increase indent — the classic outdent/indent line-art
        // arrows (⇤ / ⇥).
        if tool(
            ui,
            RichText::new("\u{21E4}").size(Style::BODY),
            "Decrease Indent",
            enabled,
        ) {
            action = Some(MenuAction::Indent(-1));
        }
        if tool(
            ui,
            RichText::new("\u{21E5}").size(Style::BODY),
            "Increase Indent",
            enabled,
        ) {
            action = Some(MenuAction::Indent(1));
        }

        // EDTB-4 — the compact `»` overflow carrying the folded Style dropdown,
        // still fully reachable (§7). Greyed with no document, like the inline
        // dropdown it replaces.
        if compact {
            ui.separator();
            if let Some(picked) = overflow(ui, cx, enabled) {
                action = Some(picked);
            }
        }
    });
    action
}

/// Render the paragraph **Style** dropdown (Normal + Heading 1-6) inline, greyed
/// with no document; its selected text reads back the caret line's current level
/// so it reflects the document (Word's Style box behavior). Returns the picked
/// [`MenuAction::Heading`].
fn style_dropdown(ui: &mut Ui, cx: &MenuContext, enabled: bool) -> Option<MenuAction> {
    let level = usize::from(cx.heading_level.unwrap_or(0)).min(STYLE_LABELS.len() - 1);
    let mut action = None;
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt("editor-style")
            .selected_text(STYLE_LABELS[level])
            .show_ui(ui, |ui| {
                for (i, label) in STYLE_LABELS.iter().enumerate() {
                    if ui.selectable_label(i == level, *label).clicked() {
                        action = Some(MenuAction::Heading(u8::try_from(i).unwrap_or(0)));
                    }
                }
            })
            .response
            .on_hover_text("Paragraph style");
    });
    action
}

/// The compact `»` overflow menu carrying the folded paragraph **Style** levels
/// (Normal + Heading 1-6) — the same [`STYLE_LABELS`] set the inline dropdown
/// offers, emitting the same [`MenuAction::Heading`] (§6), so leaning out the
/// strip loses no style (§7). Greyed with no document, exactly like the inline
/// dropdown. Reads back the caret line's level for the check-mark.
fn overflow(ui: &mut Ui, cx: &MenuContext, enabled: bool) -> Option<MenuAction> {
    let level = usize::from(cx.heading_level.unwrap_or(0)).min(STYLE_LABELS.len() - 1);
    let mut action = None;
    ui.add_enabled_ui(enabled, |ui| {
        ui.menu_button(OVERFLOW_GLYPH, |ui| {
            ui.label(
                RichText::new("Paragraph style")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            for (i, label) in STYLE_LABELS.iter().enumerate() {
                if ui.selectable_label(i == level, *label).clicked() {
                    action = Some(MenuAction::Heading(u8::try_from(i).unwrap_or(0)));
                    ui.close_menu();
                }
            }
        })
        .response
        .on_hover_text("More formatting");
    });
    action
}

/// Render Word's hover **grid-picker** inside `ui` (already inside the Insert
/// Table dialog window). Returns `Some((rows, cols))` — both 1-based — when the
/// operator clicks a cell, else `None`.
///
/// Hovering a cell highlights the whole top-left block up to it (the Word
/// drag-grid) and labels the block "C × R Table"; clicking commits that size.
pub fn table_grid(ui: &mut Ui) -> Option<(u8, u8)> {
    let cell = Style::SP_M; // one square's edge — a shared spacing token (§4)
    let gap = Style::SP_XS; // inter-cell gap
    let step = cell + gap;
    let (w, h) = (
        PICKER_COLS as f32 * cell + (PICKER_COLS as f32 - 1.0) * gap,
        PICKER_ROWS as f32 * cell + (PICKER_ROWS as f32 - 1.0) * gap,
    );
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());

    // The hovered cell (0-based), and thus the highlighted block (0..=r, 0..=c).
    let hovered = resp.hover_pos().map(|p| {
        let rel = p - rect.min;
        let c = ((rel.x / step) as usize).min(PICKER_COLS - 1);
        let r = ((rel.y / step) as usize).min(PICKER_ROWS - 1);
        (r, c)
    });

    // Clone the painter so the immutable `ui` borrow ends before the caption's
    // `ui.label` below (drawing via the clone paints the same layer).
    let painter = ui.painter().clone();
    for r in 0..PICKER_ROWS {
        for c in 0..PICKER_COLS {
            let origin = pos2(rect.min.x + c as f32 * step, rect.min.y + r as f32 * step);
            let cell_rect = Rect::from_min_size(origin, vec2(cell, cell));
            let inside = hovered.is_some_and(|(hr, hc)| r <= hr && c <= hc);
            let fill = if inside {
                Style::ACCENT
            } else {
                Style::SURFACE_HI
            };
            painter.rect_filled(cell_rect, 0.0, fill);
            painter.rect_stroke(
                cell_rect,
                0.0,
                Stroke::new(1.0, Style::BORDER),
                egui::StrokeKind::Inside,
            );
        }
    }

    // The "C × R Table" caption (Word shows columns × rows), or a hint.
    let caption = hovered.map_or_else(
        || "Drag to size the table".to_owned(),
        |(r, c)| format!("{} \u{00D7} {} Table", c + 1, r + 1),
    );
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new(caption)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );

    if resp.clicked() {
        // 1-based rows×cols; `insert_table` reads rows as body rows, cols as
        // columns (it adds the header + separator itself).
        hovered.map(|(r, c)| {
            (
                u8::try_from(r + 1).unwrap_or(1),
                u8::try_from(c + 1).unwrap_or(1),
            )
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{ListStyle, MenuAction, WrapMarker, PICKER_COLS, PICKER_ROWS, STYLE_LABELS};

    #[test]
    fn style_labels_cover_normal_plus_six_headings() {
        assert_eq!(STYLE_LABELS.len(), 7, "Normal + Heading 1-6");
        assert_eq!(STYLE_LABELS[0], "Normal");
        assert_eq!(STYLE_LABELS[6], "Heading 6");
    }

    #[test]
    fn wrap_markers_are_the_markdown_syntax() {
        // The strip's B/I/U/S emit exactly the markers the engine wraps with.
        assert_eq!(WrapMarker::Bold.marker(), "**");
        assert_eq!(WrapMarker::Italic.marker(), "*");
        assert_eq!(WrapMarker::Underline.marker(), "<u>");
        assert_eq!(WrapMarker::Strike.marker(), "~~");
    }

    #[test]
    fn list_and_indent_actions_are_distinct() {
        assert_ne!(
            MenuAction::List(ListStyle::Bullet),
            MenuAction::List(ListStyle::Numbered)
        );
        assert_ne!(MenuAction::Indent(-1), MenuAction::Indent(1));
    }

    #[test]
    fn the_picker_grid_is_a_compact_hover_grid() {
        // A small, fixed grid (Word grows on demand; this cap keeps the popup
        // compact). Runtime read of the consts, not a constant assertion.
        let (rows, cols) = (PICKER_ROWS, PICKER_COLS);
        assert!((1..=10).contains(&rows), "rows {rows} out of range");
        assert!((1..=10).contains(&cols), "cols {cols} out of range");
    }
}
