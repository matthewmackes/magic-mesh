//! The session view — what the central panel shows when the chrome bar is
//! collapsed.
//!
//! A real session is a fullscreen VM desktop rendered as an egui texture by
//! `mde-vdi` (a later unit). Until one is connected this is an honest EmptyState
//! — not a placeholder render of a fake desktop (governance §7). The monitor
//! glyph is line-drawn (no version-sensitive corner-radius type) and themed
//! entirely through the shared `Style`.

use mde_egui::egui::{self, Rect, RichText, Sense, Stroke};
use mde_egui::Style;

/// Render the no-session EmptyState, vertically centred in the available space.
pub(crate) fn show(ui: &mut egui::Ui) {
    empty_state(
        ui,
        "No active session",
        "Connect a desktop — your session appears here.",
    );
}

/// A centred EmptyState: a drawn monitor glyph over a title and a dim subtitle.
fn empty_state(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    // Push the block toward the vertical middle of the panel. The block is the
    // glyph + title + subtitle stack — roughly four XL steps tall.
    let lead = (ui.available_height() - Style::SP_XL * 4.0).max(Style::SP_XL) * 0.5;
    ui.add_space(lead);

    ui.vertical_centered(|ui| {
        let glyph = egui::vec2(Style::SP_XL * 2.5, Style::SP_XL * 2.0);
        let (area, _) = ui.allocate_exact_size(glyph, Sense::hover());
        let painter = ui.painter().clone();
        draw_monitor(&painter, area);

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

/// Draw a simple monitor outline within `area` using line segments only — no
/// rect/corner-radius API, so it is robust across egui point releases.
fn draw_monitor(painter: &egui::Painter, area: Rect) {
    let stroke = Stroke::new(2.0, Style::TEXT_DIM);
    let inset = Style::SP_XS;
    let screen = Rect::from_min_max(
        egui::pos2(area.left() + inset, area.top()),
        egui::pos2(area.right() - inset, area.top() + area.height() * 0.64),
    );

    // Screen outline (four edges).
    let tl = screen.left_top();
    let tr = screen.right_top();
    let bl = screen.left_bottom();
    let br = screen.right_bottom();
    painter.line_segment([tl, tr], stroke);
    painter.line_segment([tr, br], stroke);
    painter.line_segment([br, bl], stroke);
    painter.line_segment([bl, tl], stroke);

    // Neck + base stand.
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
