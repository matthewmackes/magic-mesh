use mde_egui::egui::{self, RichText, Stroke, Ui};
use mde_egui::Style;

const EDITOR_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

pub(crate) fn editor_tooltip(ui: &mut Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let surface = Style::resolve_color(&ctx, Style::SURFACE);
    let border = Style::resolve_color(&ctx, Style::BORDER);
    let text_color = Style::resolve_color(&ctx, Style::TEXT);
    egui::Frame::NONE
        .fill(surface)
        .stroke(egui::Stroke::new(Style::STROKE_HAIRLINE, border))
        .corner_radius(mde_egui::corner(Style::RADIUS_S))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(EDITOR_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(text_color)).wrap(),
            );
        });
}

pub(crate) fn editor_hover_text(
    response: egui::Response,
    text: impl Into<String>,
) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| editor_tooltip(ui, text.as_str()))
}

pub(crate) fn editor_menu_button<R>(
    ui: &mut Ui,
    title: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ui.menu_button(title, |ui| editor_popup_visual_scope(ui, add_contents))
}

pub(crate) fn editor_popup_visual_scope<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let previous_style = ui.ctx().style();
    let mut popup_style = (*previous_style).clone();
    apply_editor_popup_style(ui.ctx(), &mut popup_style);
    ui.ctx().set_style(popup_style);
    let inner = ui
        .scope(|ui| {
            let ctx = ui.ctx().clone();
            apply_editor_popup_style(&ctx, ui.style_mut());
            add_contents(ui)
        })
        .inner;
    ui.ctx().set_style(previous_style);
    inner
}

fn apply_editor_popup_style(ctx: &egui::Context, style: &mut egui::Style) {
    let palette = Style::current_palette(ctx);
    let accent = Style::resolve_color(ctx, Style::ACCENT);
    let border = Stroke::new(Style::STROKE_HAIRLINE, palette.border);
    let text = Stroke::new(Style::STROKE_HAIRLINE, palette.text);
    let text_dim = Stroke::new(Style::STROKE_HAIRLINE, palette.text_dim);
    let visuals = &mut style.visuals;

    visuals.window_fill = palette.surface;
    visuals.panel_fill = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.extreme_bg_color = palette.bg;
    visuals.window_stroke = border;
    visuals.override_text_color = Some(palette.text);
    visuals.menu_corner_radius = mde_egui::corner(Style::RADIUS_S);

    visuals.widgets.noninteractive.bg_fill = palette.surface;
    visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
    visuals.widgets.noninteractive.bg_stroke = border;
    visuals.widgets.noninteractive.fg_stroke = text_dim;

    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = border;
    visuals.widgets.inactive.fg_stroke = text;

    visuals.widgets.hovered.bg_fill = palette.surface_hi;
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hi;
    visuals.widgets.hovered.bg_stroke = Stroke::new(Style::STROKE_HAIRLINE, accent);
    visuals.widgets.hovered.fg_stroke = text;

    visuals.widgets.active.bg_fill = palette.surface_hi;
    visuals.widgets.active.weak_bg_fill = palette.surface_hi;
    visuals.widgets.active.bg_stroke = Stroke::new(Style::STROKE_HAIRLINE, accent);
    visuals.widgets.active.fg_stroke = text;

    visuals.widgets.open.bg_fill = palette.surface_hi;
    visuals.widgets.open.weak_bg_fill = palette.surface_hi;
    visuals.widgets.open.bg_stroke = border;
    visuals.widgets.open.fg_stroke = text;

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.selection.stroke = Stroke::new(Style::STROKE_HAIRLINE, accent);
    style.spacing.button_padding = egui::vec2(Style::SP_S, Style::CONTROL_PAD_Y);
    style.spacing.item_spacing = egui::vec2(Style::SP_XS, Style::TOOLBAR_INSET_Y);
}

#[cfg(test)]
mod tests {
    use super::{apply_editor_popup_style, editor_tooltip};
    use mde_egui::{egui, Density, Style, StyleColorScheme};

    fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill != egui::Color32::TRANSPARENT => {
                    out.push(rect.fill);
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render_tooltip(ctx: &egui::Context, text: &str) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(360.0, 96.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        editor_tooltip(ui, text);
                    });
            },
        )
    }

    #[test]
    fn editor_tooltip_uses_themed_text_and_surface_in_light_mode() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::Light, Density::Mouse);
        let expected_text = Style::resolve_color(&ctx, Style::TEXT);
        let expected_surface = Style::resolve_color(&ctx, Style::SURFACE);
        let out = render_tooltip(&ctx, "Go to line 12");

        let texts = painted_text_colors(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == "Go to line 12" && *color == expected_text),
            "Editor tooltip should paint themed text: {texts:?}"
        );
        assert_ne!(
            expected_text, expected_surface,
            "Editor tooltip text and surface must stay distinct in light mode"
        );

        let fills = rect_fills(&out.shapes);
        assert!(
            fills.contains(&expected_surface),
            "Editor tooltip should paint its themed surface: {fills:?}"
        );
    }

    #[test]
    fn editor_popup_visuals_use_themed_text_and_surface() {
        for scheme in [StyleColorScheme::Dark, StyleColorScheme::Light] {
            let ctx = egui::Context::default();
            Style::install_color_scheme_with_density(&ctx, scheme, Density::Mouse);
            let palette = Style::palette_for(scheme);
            let accent = Style::resolve_color(&ctx, Style::ACCENT);
            let mut style = (*ctx.style()).clone();

            apply_editor_popup_style(&ctx, &mut style);

            assert_eq!(style.visuals.window_fill, palette.surface);
            assert_eq!(style.visuals.panel_fill, palette.surface);
            assert_eq!(style.visuals.window_stroke.color, palette.border);
            assert_eq!(style.visuals.override_text_color, Some(palette.text));
            assert_eq!(
                style.visuals.widgets.noninteractive.fg_stroke.color,
                palette.text_dim
            );
            assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, palette.text);
            assert_eq!(style.visuals.widgets.hovered.bg_fill, palette.surface_hi);
            assert_eq!(style.visuals.widgets.hovered.bg_stroke.color, accent);
            assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, palette.text);
            assert_eq!(style.visuals.widgets.open.bg_fill, palette.surface_hi);
            assert_eq!(style.visuals.widgets.open.fg_stroke.color, palette.text);
            assert_eq!(style.spacing.button_padding.y, Style::CONTROL_PAD_Y);
            assert_eq!(style.spacing.item_spacing.y, Style::TOOLBAR_INSET_Y);
        }
    }
}
