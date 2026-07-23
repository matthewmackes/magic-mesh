//! Shared honest-empty presentation primitives.

use mde_egui::egui::{self, Rect, RichText, Sense, Stroke};
use mde_egui::Style;

/// Paint a centered empty-state story without inventing runtime content.
pub(crate) fn show(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let lead = (ui.available_height() - Style::SP_XL * 4.0).max(Style::SP_XL) * 0.5;
    ui.add_space(lead);

    ui.vertical_centered(|ui| {
        let glyph = egui::vec2(Style::SP_XL * 2.5, Style::SP_XL * 2.0);
        let (area, _) = ui.allocate_exact_size(glyph, Sense::hover());
        draw_monitor(&ui.painter().clone(), area);

        ui.add_space(Style::SP_M);
        ui.label(
            RichText::new(title)
                .color(Style::TEXT)
                .size(Style::HEADING)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(subtitle)
                .color(Style::TEXT_DIM)
                .size(Style::BODY),
        );
    });
}

/// Draw the shared monitor placeholder used by chooser and empty-state views.
pub(crate) fn draw_monitor(painter: &egui::Painter, area: Rect) {
    let stroke = Stroke::new(2.0, Style::TEXT_DIM);
    let inset = Style::SP_XS;
    let screen = Rect::from_min_max(
        egui::pos2(area.left() + inset, area.top()),
        egui::pos2(area.right() - inset, area.top() + area.height() * 0.64),
    );

    let tl = screen.left_top();
    let tr = screen.right_top();
    let bl = screen.left_bottom();
    let br = screen.right_bottom();
    painter.line_segment([tl, tr], stroke);
    painter.line_segment([tr, br], stroke);
    painter.line_segment([br, bl], stroke);
    painter.line_segment([bl, tl], stroke);

    let cx = screen.center().x;
    painter.line_segment(
        [
            egui::pos2(cx, screen.bottom()),
            egui::pos2(cx, area.bottom() - Style::SP_XS),
        ],
        stroke,
    );
    let half = area.width() * 0.22;
    painter.line_segment(
        [
            egui::pos2(cx - half, area.bottom()),
            egui::pos2(cx + half, area.bottom()),
        ],
        stroke,
    );
}
