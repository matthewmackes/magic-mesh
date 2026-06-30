//! `Style` — the single source of look for every E12 surface (governance §4, lock 9).
//!
//! A Rust module, not a token crate: there is deliberately **no raw-literal lint
//! gate** (the §0-Simple lever), so this module *is* the discipline. Surfaces read
//! these constants and call [`Style::install`]; they never hand-roll a colour or a
//! spacing value.
//!
//! The palette is the **Quasar dark** identity (the platform default). Values
//! are render-agnostic data, so they are unit-tested without a GPU.

use egui::{Color32, Context, Stroke};

/// The shared egui design system. All fields are `const` so they are usable in
/// `const` contexts and read directly at call sites.
pub struct Style;

impl Style {
    // ── Palette (Quasar dark) ───────────────────────────────────────────────
    /// Window / app background — the deepest surface.
    pub const BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1A);
    /// Raised surface — panels, cards, inputs.
    pub const SURFACE: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x25);
    /// Hovered / highlighted surface.
    pub const SURFACE_HI: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x32);
    /// Hairline borders + separators.
    pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x3D);

    /// Primary text.
    pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xEC);
    /// Secondary / dimmed text.
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0xA6);

    /// Interactive / brand accent (Quasar azure).
    pub const ACCENT: Color32 = Color32::from_rgb(0x5B, 0x8C, 0xFF);
    /// Accent, hovered.
    pub const ACCENT_HI: Color32 = Color32::from_rgb(0x7A, 0xA2, 0xFF);

    /// Status — danger / error.
    pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x5C, 0x57);
    /// Status — warning.
    pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB4, 0x54);
    /// Status — success / ok.
    pub const OK: Color32 = Color32::from_rgb(0x4F, 0xD0, 0x8A);

    // ── Spacing (8px grid; XS is the half-step) ─────────────────────────────
    /// 4px — half-step (tight insets, icon gaps).
    pub const SP_XS: f32 = 4.0;
    /// 8px — base unit.
    pub const SP_S: f32 = 8.0;
    /// 16px.
    pub const SP_M: f32 = 16.0;
    /// 24px.
    pub const SP_L: f32 = 24.0;
    /// 32px.
    pub const SP_XL: f32 = 32.0;

    /// Corner radius for surfaces (data; applied by surfaces at draw time so the
    /// harness build stays free of egui's version-sensitive corner-radius type).
    pub const RADIUS: f32 = 6.0;

    // ── Type scale (point sizes) ────────────────────────────────────────────
    /// Small / caption text.
    pub const SMALL: f32 = 12.0;
    /// Body text.
    pub const BODY: f32 = 14.0;
    /// Section heading.
    pub const HEADING: f32 = 22.0;

    /// Install the shared look on an egui [`Context`]. Call once per surface,
    /// from the harness runner's creation hook (see [`crate::run_client`]).
    pub fn install(ctx: &Context) {
        // Fira Code is the default font set for every surface (governance §4).
        crate::fonts::install(ctx);

        let mut v = egui::Visuals::dark();

        v.panel_fill = Self::BG;
        v.window_fill = Self::SURFACE;
        v.extreme_bg_color = Self::BG;
        v.faint_bg_color = Self::SURFACE;
        v.override_text_color = Some(Self::TEXT);
        v.hyperlink_color = Self::ACCENT;
        v.window_stroke = Stroke::new(1.0, Self::BORDER);
        v.selection.bg_fill = Self::ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, Self::ACCENT);

        let border = Stroke::new(1.0, Self::BORDER);
        let text = Stroke::new(1.0, Self::TEXT);
        let text_dim = Stroke::new(1.0, Self::TEXT_DIM);

        // Non-interactive chrome (labels, separators).
        v.widgets.noninteractive.bg_fill = Self::BG;
        v.widgets.noninteractive.weak_bg_fill = Self::BG;
        v.widgets.noninteractive.bg_stroke = border;
        v.widgets.noninteractive.fg_stroke = text_dim;

        // Resting interactive widgets.
        v.widgets.inactive.bg_fill = Self::SURFACE;
        v.widgets.inactive.weak_bg_fill = Self::SURFACE;
        v.widgets.inactive.bg_stroke = border;
        v.widgets.inactive.fg_stroke = text;

        // Hover.
        v.widgets.hovered.bg_fill = Self::SURFACE_HI;
        v.widgets.hovered.weak_bg_fill = Self::SURFACE_HI;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Self::ACCENT);
        v.widgets.hovered.fg_stroke = text;

        // Pressed / active.
        v.widgets.active.bg_fill = Self::ACCENT;
        v.widgets.active.weak_bg_fill = Self::ACCENT;
        v.widgets.active.bg_stroke = Stroke::new(1.0, Self::ACCENT_HI);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Self::BG);

        ctx.style_mut(|s| {
            s.visuals = v;
            s.spacing.item_spacing = egui::vec2(Self::SP_S, Self::SP_S);
            s.spacing.button_padding = egui::vec2(Self::SP_M, Self::SP_S);
            s.spacing.indent = Self::SP_M;
        });
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants, clippy::float_cmp)]
mod tests {
    use super::Style;

    #[test]
    fn spacing_follows_the_8px_grid() {
        for s in [Style::SP_S, Style::SP_M, Style::SP_L, Style::SP_XL] {
            assert_eq!(s % 8.0, 0.0, "{s} is off the 8px grid");
        }
        // XS is the deliberate half-step.
        assert_eq!(Style::SP_XS, Style::SP_S / 2.0);
    }

    #[test]
    fn core_colours_are_distinct() {
        assert_ne!(Style::BG, Style::SURFACE);
        assert_ne!(Style::SURFACE, Style::SURFACE_HI);
        assert_ne!(Style::TEXT, Style::TEXT_DIM);
        assert_ne!(Style::ACCENT, Style::BG);
        assert_ne!(Style::ACCENT, Style::ACCENT_HI);
    }

    #[test]
    fn type_scale_is_ascending() {
        assert!(Style::SMALL < Style::BODY);
        assert!(Style::BODY < Style::HEADING);
    }

    #[test]
    fn install_applies_without_a_gpu() {
        // egui::Context is CPU-only; installing the style must not panic and must
        // actually land our palette on the context.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        assert_eq!(ctx.style().visuals.panel_fill, Style::BG);
        assert_eq!(ctx.style().visuals.hyperlink_color, Style::ACCENT);
        assert_eq!(ctx.style().spacing.indent, Style::SP_M);
    }
}
