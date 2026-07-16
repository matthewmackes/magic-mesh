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

/// Compact display data for a shell-wide operation progress badge.
///
/// This intentionally contains only the bounded UI projection: detailed file,
/// transfer, or storage job models stay with their owning surface/worker. Shared
/// chrome can use this primitive to render one consistent progress affordance
/// without depending on those models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperationProgressView<'a> {
    /// Active jobs represented by this summary.
    pub active: usize,
    /// Average known progress, `None` while all active jobs are still queued.
    pub fraction: Option<f32>,
    /// Already-bounded display label.
    pub label: &'a str,
}

impl<'a> OperationProgressView<'a> {
    /// Construct a compact operation-progress view.
    #[must_use]
    pub fn new(active: usize, fraction: Option<f32>, label: &'a str) -> Self {
        Self {
            active,
            fraction: fraction.map(|f| f.clamp(0.0, 1.0)),
            label,
        }
    }
}

/// Human-readable text painted inside a compact operation progress badge.
#[must_use]
pub fn operation_progress_text(progress: OperationProgressView<'_>) -> String {
    match progress.fraction {
        Some(fraction) => format!("{} - {:.0}%", progress.label, fraction * 100.0),
        None => format!("{} - starting", progress.label),
    }
}

/// AccessKit value text for a compact operation progress badge.
#[must_use]
pub fn operation_progress_value(progress: OperationProgressView<'_>) -> String {
    match progress.fraction {
        Some(fraction) => format!(
            "{} active file operation(s), {:.0}% average progress",
            progress.active,
            fraction * 100.0
        ),
        None => format!(
            "{} active file operation(s), progress pending",
            progress.active
        ),
    }
}

/// Paint a compact operation progress badge into an already-allocated rect.
///
/// Callers keep ownership of routing, interaction, and accessibility IDs; this
/// primitive owns the visual language so file/transfer/storage progress does not
/// fork into separate ad-hoc bars.
pub fn paint_operation_progress_badge(
    ui: &Ui,
    rect: egui::Rect,
    progress: OperationProgressView<'_>,
    selected: bool,
    hovered: bool,
) {
    let fill = if selected {
        Style::selection_wash()
    } else if hovered {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    ui.painter().rect_filled(rect, Style::RADIUS, fill);
    ui.painter().rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    if selected {
        let underline_h = Style::SP_XS;
        let underline = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - underline_h),
            rect.right_bottom(),
        );
        ui.painter()
            .rect_filled(underline, egui::CornerRadius::ZERO, Style::ACCENT);
    }

    let clip = rect.shrink(Style::SP_XS);
    let painter = ui.painter().with_clip_rect(clip);
    painter.text(
        egui::pos2(rect.left() + Style::SP_S, rect.top() + Style::SP_XS),
        egui::Align2::LEFT_TOP,
        operation_progress_text(progress),
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );

    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.left() + Style::SP_S, rect.bottom() - Style::SP_S - 5.0),
        egui::vec2((rect.width() - Style::SP_M).max(Style::SP_S), 5.0),
    );
    painter.rect_filled(bar, Style::RADIUS_S, Style::LAYER_01);
    painter.rect_stroke(
        bar,
        Style::RADIUS_S,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    let (fraction, color) = progress
        .fraction
        .map_or((0.14, Style::TEXT_DIM), |f| (f, Style::ACCENT));
    let fill_w = (bar.width() * fraction.clamp(0.0, 1.0)).max(Style::SP_XS);
    let filled = egui::Rect::from_min_size(bar.min, egui::vec2(fill_w, bar.height()));
    painter.rect_filled(filled, Style::RADIUS_S, color);
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

    #[test]
    fn operation_progress_badge_uses_shared_text_value_and_paints_shapes() {
        let progress = OperationProgressView::new(2, Some(0.42), "2 browser downloads");
        let progress_text = operation_progress_text(progress);
        assert_eq!(progress_text, "2 browser downloads - 42%");
        assert!(progress_text.is_ascii());
        assert_eq!(
            operation_progress_value(progress),
            "2 active file operation(s), 42% average progress"
        );
        let pending = OperationProgressView::new(1, None, "Copy report.txt");
        let pending_text = operation_progress_text(pending);
        assert_eq!(pending_text, "Copy report.txt - starting");
        assert!(pending_text.is_ascii());
        assert_eq!(
            operation_progress_value(pending),
            "1 active file operation(s), progress pending"
        );

        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 36.0), Sense::hover());
                paint_operation_progress_badge(ui, rect, progress, true, false);
            });
        });
        assert!(
            !out.shapes.is_empty(),
            "operation progress badge must paint visible shapes"
        );
    }
}
