//! The persistent chrome bar — the thin top strip that frames every session and
//! carries the Expand toggle into the Workbench.
//!
//! E12-3 ships honest *placeholders*: Peers / Sessions / Status read no Bus yet,
//! so each shows a neutral status dot and an em-dash rather than a fabricated
//! count (governance §7). The Bus wiring lands with the panels.

use mde_egui::egui::{self, Align, Layout, RichText, Sense};
use mde_egui::Style;

/// The three status placeholders the chrome bar carries until the Bus is wired.
const STATUS_SLOTS: [&str; 3] = ["Peers", "Sessions", "Status"];

/// Render the chrome bar's contents inside a top panel. Returns `true` when the
/// Expand/Collapse toggle was clicked this frame.
pub(crate) fn show(ui: &mut egui::Ui, expanded: bool) -> bool {
    let mut toggled = false;
    ui.horizontal_centered(|ui| {
        // Brand mark — keeps the bar identifiable when a session is fullscreen.
        ui.label(
            RichText::new("MCNF")
                .color(Style::ACCENT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_M);

        for (i, slot) in STATUS_SLOTS.iter().enumerate() {
            if i > 0 {
                ui.add_space(Style::SP_S);
                ui.label(RichText::new("·").color(Style::BORDER).size(Style::BODY));
                ui.add_space(Style::SP_S);
            }
            status_slot(ui, slot);
        }

        // Expand / Collapse, pinned to the right edge.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if expanded { "Collapse" } else { "Expand" };
            if ui.button(label).clicked() {
                toggled = true;
            }
        });
    });
    toggled
}

/// One status placeholder: a neutral status dot, the slot name, and an em-dash
/// standing in for the not-yet-wired value.
fn status_slot(ui: &mut egui::Ui, name: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 3.5, Style::TEXT_DIM);
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new(name).color(Style::TEXT).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new("—").color(Style::TEXT_DIM).size(Style::SMALL));
}

#[cfg(test)]
mod tests {
    #[test]
    fn carries_the_three_status_placeholders() {
        assert_eq!(super::STATUS_SLOTS, ["Peers", "Sessions", "Status"]);
    }
}
