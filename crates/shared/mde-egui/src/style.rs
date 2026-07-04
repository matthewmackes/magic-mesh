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

use crate::formfactor::Formfactor;

/// The **interaction density** of a surface — how large hit targets and spacing are
/// (SURFACE-11, design lock 16).
///
/// A pointer (laptop) wants the compact desktop metrics; a finger (tablet) wants larger
/// targets and more breathing room. The shell installs the density
/// [`for the current formfactor`](Density::for_formfactor) and re-installs it on every
/// Tablet↔Laptop flip, so the same UI grows under touch and reverts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// Pointer density — the compact desktop metrics (laptop / windowed fallback).
    #[default]
    Mouse,
    /// Touch density — larger hit targets and wider spacing (tablet formfactor).
    Touch,
}

impl Density {
    /// The density a formfactor engages: Tablet → [`Touch`](Self::Touch), Laptop →
    /// [`Mouse`](Self::Mouse). The single mapping the shell keys off the SURFACE-9
    /// signal.
    #[must_use]
    pub const fn for_formfactor(formfactor: Formfactor) -> Self {
        match formfactor {
            Formfactor::Tablet => Self::Touch,
            Formfactor::Laptop => Self::Mouse,
        }
    }

    /// The minimum interactive **hit-target** height in points. Touch grows it to a
    /// finger-sized target (the ~44 pt touch-target convention); mouse keeps the
    /// compact control height.
    #[must_use]
    pub const fn min_hit_target(self) -> f32 {
        match self {
            Self::Mouse => 24.0,
            Self::Touch => 44.0,
        }
    }

    /// The multiplier applied to the base 8px spacing grid (item spacing, button
    /// padding, indent) so a touch surface has more breathing room between targets.
    #[must_use]
    pub const fn spacing_scale(self) -> f32 {
        match self {
            Self::Mouse => 1.0,
            Self::Touch => 1.5,
        }
    }
}

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

    // ── Carbon elevation layers ─────────────────────────────────────────────
    // The Carbon "layer" model for nested regions: a page rests one tonal step
    // above the window [`BG`](Self::BG), and a card rests one step above the page —
    // regions separate by elevation, not a heavy border. Named aliases over the
    // existing surface palette (one palette, no new hue §4): the two steps a
    // two-level layout (a page + its section cards, SETTINGS-2) needs, reusable
    // shell-wide.
    /// Carbon elevation — **layer-01**: a page / panel one step above [`BG`](Self::BG).
    pub const LAYER_01: Color32 = Self::SURFACE;
    /// Carbon elevation — **layer-02**: a card resting on [`LAYER_01`](Self::LAYER_01).
    pub const LAYER_02: Color32 = Self::SURFACE_HI;

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

    // ── Categorical accents (picker groups · explorer categories) ───────────
    // Six distinct Carbon **categorical** hues that key a group's / category's
    // identity — the ONE colour language shared across the bottom picker's group
    // labels (PICKER-2, design L4) and the unit explorer's per-category accent
    // (EXPLORER-15, design O8). Defined **once** here so both surfaces speak the
    // same palette (§4 — the raw hex lives only in this token module, never at a
    // call site). These are display accents for categorisation, distinct from the
    // single interactive brand [`ACCENT`](Self::ACCENT).
    /// Categorical accent — **Comms** (Carbon cyan).
    pub const ACCENT_COMMS: Color32 = Color32::from_rgb(0x33, 0xB1, 0xFF);
    /// Categorical accent — **Workloads** (Carbon purple).
    pub const ACCENT_WORKLOADS: Color32 = Color32::from_rgb(0xA5, 0x6E, 0xFF);
    /// Categorical accent — **Terminals** (Carbon teal).
    pub const ACCENT_TERMINALS: Color32 = Color32::from_rgb(0x08, 0xBD, 0xBA);
    /// Categorical accent — **Mesh** (Carbon green).
    pub const ACCENT_MESH: Color32 = Color32::from_rgb(0x42, 0xBE, 0x65);
    /// Categorical accent — **System** (Carbon gold).
    pub const ACCENT_SYSTEM: Color32 = Color32::from_rgb(0xF1, 0xC2, 0x1B);
    /// Categorical accent — **Media** (Carbon magenta).
    pub const ACCENT_MEDIA: Color32 = Color32::from_rgb(0xFF, 0x7E, 0xB6);

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

    /// Install the shared look on an egui [`Context`] at the default (pointer)
    /// density. Call once per surface, from the harness runner's creation hook (see
    /// [`crate::run_client`]).
    pub fn install(ctx: &Context) {
        Self::install_with_density(ctx, Density::Mouse);
    }

    /// Install the shared look at an explicit [`Density`] (SURFACE-11, lock 16). The
    /// palette/type scale are identical across densities — only the interaction
    /// metrics (hit-target size + spacing) grow under [`Density::Touch`], so the shell
    /// can flip Tablet↔Laptop by re-installing at the new density.
    pub fn install_with_density(ctx: &Context, density: Density) {
        // Droid Sans Mono is the default font set for every surface (governance §4).
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

        // SURFACE-11: the density scales the interaction metrics — targets and spacing
        // grow under touch, the palette does not.
        let sp = density.spacing_scale();
        ctx.style_mut(|s| {
            s.visuals = v;
            s.spacing.item_spacing = egui::vec2(Self::SP_S * sp, Self::SP_S * sp);
            s.spacing.button_padding = egui::vec2(Self::SP_M * sp, Self::SP_S * sp);
            s.spacing.indent = Self::SP_M * sp;
            // The minimum interactive size is the finger/pointer hit target.
            s.spacing.interact_size.y = density.min_hit_target();
        });
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants, clippy::float_cmp)]
mod tests {
    use super::{Density, Style};
    use crate::formfactor::Formfactor;

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
    fn categorical_accents_are_a_distinct_palette() {
        // PICKER-2 / EXPLORER-15 O8: the six shared picker-group / explorer-category
        // accents must be mutually distinguishable — one colour language, six hues —
        // and each set apart from the single interactive brand accent so a category
        // tint never reads as an interaction affordance.
        let cats = [
            Style::ACCENT_COMMS,
            Style::ACCENT_WORKLOADS,
            Style::ACCENT_TERMINALS,
            Style::ACCENT_MESH,
            Style::ACCENT_SYSTEM,
            Style::ACCENT_MEDIA,
        ];
        for (i, a) in cats.iter().enumerate() {
            assert_ne!(
                *a,
                Style::ACCENT,
                "a categorical accent must differ from the brand accent"
            );
            for b in &cats[i + 1..] {
                assert_ne!(a, b, "categorical accents must be mutually distinct");
            }
        }
    }

    #[test]
    fn carbon_elevation_layers_form_an_ascending_ladder() {
        // The Carbon layer ladder for nested regions: the window BG, then a page
        // (layer-01), then a card (layer-02) — each a distinct tonal step so a card
        // reads as raised without a heavy fill, and both layers resolve onto the
        // existing surface palette (one palette, §4 — no new hue minted).
        assert_ne!(Style::BG, Style::LAYER_01);
        assert_ne!(Style::LAYER_01, Style::LAYER_02);
        assert_eq!(Style::LAYER_01, Style::SURFACE);
        assert_eq!(Style::LAYER_02, Style::SURFACE_HI);
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

    // --- SURFACE-11: touch density -------------------------------------------------

    #[test]
    fn touch_density_grows_hit_targets_and_spacing() {
        // The whole point of the touch density: bigger targets, more spacing.
        assert!(
            Density::Touch.min_hit_target() > Density::Mouse.min_hit_target(),
            "touch hit targets must be larger than pointer ones"
        );
        assert!(
            Density::Touch.spacing_scale() > Density::Mouse.spacing_scale(),
            "touch spacing must be wider than pointer spacing"
        );
    }

    #[test]
    fn density_is_selected_by_formfactor() {
        // The single mapping the shell keys off the SURFACE-9 signal.
        assert_eq!(Density::for_formfactor(Formfactor::Tablet), Density::Touch);
        assert_eq!(Density::for_formfactor(Formfactor::Laptop), Density::Mouse);
    }

    #[test]
    fn installing_touch_density_enlarges_the_context_metrics() {
        // Runtime-observable: the same install path, at Touch density, lands larger
        // interaction metrics on the egui context than the Mouse (default) density.
        let mouse = egui::Context::default();
        Style::install_with_density(&mouse, Density::Mouse);
        let touch = egui::Context::default();
        Style::install_with_density(&touch, Density::Touch);

        assert!(
            touch.style().spacing.interact_size.y > mouse.style().spacing.interact_size.y,
            "touch hit target grew on the context"
        );
        assert!(
            touch.style().spacing.item_spacing.x > mouse.style().spacing.item_spacing.x,
            "touch spacing grew on the context"
        );
        assert!(
            touch.style().spacing.button_padding.x > mouse.style().spacing.button_padding.x,
            "touch button padding grew on the context"
        );
        // Bare `install` is the pointer density (unchanged default).
        let d = egui::Context::default();
        Style::install(&d);
        assert_eq!(
            d.style().spacing.interact_size.y,
            mouse.style().spacing.interact_size.y
        );
    }
}
