//! Shared egui widgets built on the single-source [`crate::Style`].
//!
//! The E12 equivalent of the retired `mde-theme::components`: surfaces reuse
//! these instead of re-typing the same idiom, so a look lives in ONE place
//! (§6 glue; `/polish` axis 7 — component reuse & consolidation).

use egui::{Color32, Response, RichText, Sense, Ui};

use crate::Style;

/// A **muted note** — a dim, small-caption label — returning its [`Response`].
///
/// This is the single source for the "honestly empty / not-yet-reported" panel
/// state (and any small secondary caption): [`Style::TEXT_DIM`] at the
/// [`Style::SMALL`] type size. It consolidates the
/// `ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL))`
/// idiom that was hand-rolled across every surface — restyle the empty/caption
/// look in one place now, and every panel follows.
///
/// Use it for a genuinely-empty state (no data yet), NOT as a stand-in for real
/// content (§7 — a muted note is an honest "nothing here", never a mockup).
pub fn muted_note(ui: &mut Ui, msg: impl Into<String>) -> Response {
    ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL))
}

/// A small filled **status dot** — a [`Style::SP_S`]-sized circle in `color` —
/// for an inline health/presence indicator beside a label.
///
/// The single source for the primitive that mde-files-egui and mde-voice-egui
/// hand-rolled byte-identically. `color` is a `Style` palette token
/// ([`Style::OK`]/[`Style::WARN`]/[`Style::DANGER`]/…), never a raw literal.
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let diameter = Style::SP_S;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter, diameter), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter * 0.28, color);
}

/// A **labelled value row** on the spacing grid — a dim [`Style::SMALL`] `label`,
/// a [`Style::SP_S`] gutter, then a `tone`-coloured `value` at the same size.
///
/// The single source for the "field" row that several shell panels hand-rolled
/// byte-identically. `tone` is a `Style` palette token (never a raw literal).
pub fn field(ui: &mut Ui, label: &str, value: &str, tone: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(tone, RichText::new(value).size(Style::SMALL));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_note_renders_a_real_small_dim_label() {
        // Headless render: the widget must lay out a real caption (non-zero
        // height, no panic) — proving it's a live render, not dead code — and it
        // reads the shared Style rather than a hand-rolled colour/size.
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = muted_note(ui, "not yet reported");
                assert!(r.rect.height() > 0.0, "muted_note laid out nothing");
                // Accepts both &str and String callers (the two call-site shapes).
                let owned = String::from("roster not yet reported");
                let _ = muted_note(ui, owned);
            });
        });
    }

    #[test]
    fn status_dot_paints_without_panicking() {
        // Headless render: allocating + painting the dot must not panic, and it
        // takes a Style palette token (proving the primitive is live, reachable
        // code both surfaces can share).
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                status_dot(ui, Style::OK);
                status_dot(ui, Style::DANGER);
                // The labelled-value row lays out (dim label + toned value).
                field(ui, "role", "lighthouse", Style::OK);
            });
        });
    }
}
