//! Terminal-local hover cards shared by the tab bar, layouts, and tmux chrome.

use mde_egui::egui::{self, RichText};
use mde_egui::Style;

const TERMINAL_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

pub(crate) fn terminal_tooltip(ui: &mut egui::Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let surface = Style::resolve_color(&ctx, Style::SURFACE);
    let border = Style::resolve_color(&ctx, Style::BORDER);
    let text_color = Style::resolve_color(&ctx, Style::TEXT);
    egui::Frame::NONE
        .fill(surface)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(Style::RADIUS as u8))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(TERMINAL_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(text_color)).wrap(),
            );
        });
}

pub(crate) fn terminal_hover_text(
    response: egui::Response,
    text: impl Into<String>,
) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| terminal_tooltip(ui, text.as_str()))
}

#[cfg(test)]
mod tests {
    use super::terminal_tooltip;
    use mde_egui::egui;
    use mde_egui::{Density, Style, StyleColorScheme};

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

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
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

    fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    if rect.fill != egui::Color32::TRANSPARENT {
                        out.push(rect.fill);
                    }
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

    fn render_terminal_tooltip_frame(ctx: &egui::Context, text: &str) -> egui::FullOutput {
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
                        terminal_tooltip(ui, text);
                    });
            },
        )
    }

    #[test]
    fn terminal_tooltip_uses_themed_text_and_surface_in_light_mode() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::Light, Density::Mouse);
        let tooltip = "New terminal on a mesh node";
        let out = render_terminal_tooltip_frame(&ctx, tooltip);
        let text_color = Style::resolve_color(&ctx, Style::TEXT);
        let surface = Style::resolve_color(&ctx, Style::SURFACE);

        let texts = painted_text_colors(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == tooltip && *color == text_color),
            "Terminal tooltip should paint themed text: {texts:?}"
        );
        assert!(
            text_color != surface,
            "Terminal tooltip text and surface must stay distinct in light mode"
        );

        let fills = painted_fill_colors(&out.shapes);
        assert!(
            fills.contains(&surface),
            "Terminal tooltip should paint its themed surface: {fills:?}"
        );
    }
}
