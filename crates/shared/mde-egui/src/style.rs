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
    /// **Emphasis** text — one rung brighter than [`TEXT`](Self::TEXT) (Carbon
    /// white). The single embedded face (Droid Sans Mono, [`crate::fonts`]) has
    /// no bold cut, so a *bold* / heading run cannot render heavier glyphs; the
    /// honest token cue for weight on the dark ground is this brighter tone, the
    /// mirror of [`TEXT_DIM`](Self::TEXT_DIM)'s dimmer one. Markdown preview
    /// (EDTB-7) paints bold spans + heading titles with it.
    pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);

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
    /// Editorial — a **spelling** miss: the red wavy underline the editor draws
    /// under a misspelled word (EDTB-6, `mde-editor-egui`). Its own token — a
    /// deeper, more saturated Carbon red-60 — so a spell squiggle reads distinct
    /// from a [`DANGER`](Self::DANGER) *error* underline (the LSP diagnostics
    /// squiggle), never the same red for two different meanings.
    pub const SPELL: Color32 = Color32::from_rgb(0xDA, 0x1E, 0x28);

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

    // ── Node capability grade ramp (A–F, green→red) ─────────────────────────
    // NODE-GRADE-3 (design docs/design/node-grade.md #4): ONE shared A–F ramp that
    // every grade UI reads — the dock's per-node capability list (NODE-GRADE-2)
    // today, any future grade surface tomorrow. It is **not** a new palette: each
    // rung resolves onto an existing status/accent token (§4 — one palette, no raw
    // hex minted here), so the grades speak the same greens/ambers/reds the rest of
    // the shell already does. The rungs redden monotonically A→F (green · lime ·
    // gold · orange · red); [`GradeBand`] maps a 0–100 score → band → colour.
    /// Grade **A** — healthy and with headroom. The success green ([`OK`](Self::OK)).
    pub const GRADE_A: Color32 = Self::OK;
    /// Grade **B** — the brighter/limier second green rung
    /// ([`ACCENT_MESH`](Self::ACCENT_MESH), Carbon green).
    pub const GRADE_B: Color32 = Self::ACCENT_MESH;
    /// Grade **C** — the mid rung, gold/yellow ([`ACCENT_SYSTEM`](Self::ACCENT_SYSTEM)).
    pub const GRADE_C: Color32 = Self::ACCENT_SYSTEM;
    /// Grade **D** — degraded, the warning amber/orange ([`WARN`](Self::WARN)).
    pub const GRADE_D: Color32 = Self::WARN;
    /// Grade **F** — failing or maxed out, the danger red ([`DANGER`](Self::DANGER)).
    pub const GRADE_F: Color32 = Self::DANGER;

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
    /// Sub-heading (Carbon productive-heading-04) — between [`BODY`](Self::BODY)
    /// and [`HEADING`](Self::HEADING); the markdown-preview H3 rung (EDTB-7).
    pub const TITLE: f32 = 18.0;
    /// Section heading.
    pub const HEADING: f32 = 22.0;
    /// Display heading (Carbon productive-heading-06) — the largest type rung,
    /// the markdown-preview H1 title size (EDTB-7).
    pub const DISPLAY: f32 = 28.0;

    /// The point size for a markdown/rich-text heading `level` on the shared
    /// type ramp: H1 → [`DISPLAY`](Self::DISPLAY), H2 → [`HEADING`](Self::HEADING),
    /// H3 → [`TITLE`](Self::TITLE), H4–H6 → [`BODY`](Self::BODY) (distinguished
    /// by the emphasis tone, not a fourth size). Monotonic non-increasing, so a
    /// deeper heading never renders larger than a shallower one; `0` and levels
    /// past 6 clamp onto the ends. The markdown preview (`mde-editor-egui`,
    /// EDTB-7) sizes every heading through this one ramp so no literal point size
    /// leaks into the surface crate (§4).
    #[must_use]
    pub const fn heading_size(level: u8) -> f32 {
        match level {
            0 | 1 => Self::DISPLAY,
            2 => Self::HEADING,
            3 => Self::TITLE,
            _ => Self::BODY,
        }
    }

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
        v.window_stroke = Stroke::new(1.0, Self::BORDER);

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
        v.widgets.hovered.fg_stroke = text;

        // Pressed / active.
        v.widgets.active.fg_stroke = Stroke::new(1.0, Self::BG);

        // The interactive **accent** — the ONE re-tintable field group: the
        // hyperlink, the text-selection wash + ring, the hover ring, and the
        // pressed fill + ring. Applied here from the brand [`ACCENT`](Self::ACCENT);
        // a Personalization → Theme pick (SETTINGS-5) re-applies the SAME derivation
        // over the live context with a chosen accent (see [`set_accent`]), so the
        // default look + a runtime override share one source of truth (§4/§6).
        Self::accent_visuals(&mut v, Self::ACCENT, Self::ACCENT_HI);

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

    /// Apply the interactive-accent derivation to `v` — the field group a runtime
    /// accent choice re-tints (SETTINGS-5): the hyperlink colour, the text-selection
    /// wash + ring, the hover ring, and the pressed/active fill + ring. Factored so
    /// [`install_with_density`](Self::install_with_density) (the default brand accent)
    /// and [`set_accent`](Self::set_accent) (a chosen accent) share ONE derivation and
    /// can never fork the look (§4/§6). `accent_hi` is the slightly-lifted variant for
    /// the pressed ring (the brand pair passes [`ACCENT`](Self::ACCENT)/[`ACCENT_HI`](Self::ACCENT_HI)).
    fn accent_visuals(v: &mut egui::Visuals, accent: Color32, accent_hi: Color32) {
        v.hyperlink_color = accent;
        v.selection.bg_fill = accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, accent);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
        v.widgets.active.bg_fill = accent;
        v.widgets.active.weak_bg_fill = accent;
        v.widgets.active.bg_stroke = Stroke::new(1.0, accent_hi);
    }

    /// Re-tint the live interactive **accent** on `ctx` to `accent` (SETTINGS-5 — the
    /// Personalization → Theme accent choice). Re-applies ONLY the accent-derived
    /// visual fields (via [`accent_visuals`](Self::accent_visuals)) over the
    /// already-installed look, so the palette / density / type scale are untouched.
    /// The shell re-applies this each frame from its Settings poll, so a chosen accent
    /// survives a formfactor [`install_with_density`](Self::install_with_density)
    /// re-install. A user pick has no separate "hi" token, so the chosen accent doubles
    /// as the pressed-ring highlight.
    pub fn set_accent(ctx: &Context, accent: Color32) {
        ctx.style_mut(|s| Self::accent_visuals(&mut s.visuals, accent, accent));
    }

    /// The **load-bar fill** colour for a 0–100 capability score: a smooth blend
    /// along the same green→red ramp as the A–F letters (design #5 — the tiny
    /// per-node load bar). `100` → [`GRADE_A`](Self::GRADE_A) green, `0` →
    /// [`GRADE_F`](Self::GRADE_F) red, interpolated continuously between the rungs so
    /// the bar reads as a gradient while the letter stays a discrete band. Scores
    /// outside `0..=100` clamp to the ends rather than panic.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn grade_fill(score: f32) -> Color32 {
        // Rungs worst→best: a full (100) bar is green, an empty (0) bar is red.
        const RAMP: [Color32; 5] = [
            Style::GRADE_F,
            Style::GRADE_D,
            Style::GRADE_C,
            Style::GRADE_B,
            Style::GRADE_A,
        ];
        let last = RAMP.len() - 1;
        let pos = (score.clamp(0.0, 100.0) / 100.0) * last as f32;
        let lo = pos.floor();
        let idx = lo as usize;
        if idx >= last {
            return RAMP[last];
        }
        blend(RAMP[idx], RAMP[idx + 1], pos - lo)
    }
}

/// Linear-interpolate two colours in gamma space at `t` ∈ `[0, 1]` — a small local
/// mixer for [`Style::grade_fill`]'s load-bar gradient (`t = 0` → `a`, `t = 1` → `b`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| {
        (f32::from(y) - f32::from(x))
            .mul_add(t, f32::from(x))
            .round() as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// The five capability **bands** a 0–100 node score falls into — the A–F grade
/// (there is no "E"; the classic school ramp skips it).
///
/// Each band owns one colour on the shared green→red ramp
/// ([`Style::GRADE_A`]..[`Style::GRADE_F`]) and knows whether it is an alarm band, so
/// "which score is which grade" and "which grades blink" are each defined **once**.
/// The dock (NODE-GRADE-2) and any future grade UI map a score with
/// [`from_score`](Self::from_score) then read the band's [`color`](Self::color) /
/// [`letter`](Self::letter) / [`is_alert`](Self::is_alert).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradeBand {
    /// **A** — score ≥ 90: healthy and with spare headroom.
    A,
    /// **B** — score ≥ 80.
    B,
    /// **C** — score ≥ 70.
    C,
    /// **D** — score ≥ 60: degraded; an **alarm** band (blinks, design #6/#16).
    D,
    /// **F** — score < 60: failing or maxed out; an **alarm** band (blinks, #6/#16).
    F,
}

impl GradeBand {
    /// The band a 0–100 capability score falls into, on the classic **90/80/70/60**
    /// thresholds (design #9). A `NaN` score reads as the worst band
    /// ([`F`](Self::F)) — an unscored/absent node is treated as failing, not healthy.
    #[must_use]
    pub const fn from_score(score: f32) -> Self {
        if score >= 90.0 {
            Self::A
        } else if score >= 80.0 {
            Self::B
        } else if score >= 70.0 {
            Self::C
        } else if score >= 60.0 {
            Self::D
        } else {
            Self::F
        }
    }

    /// The band's colour on the shared green→red ramp ([`Style::GRADE_A`]..`GRADE_F`).
    #[must_use]
    pub const fn color(self) -> Color32 {
        match self {
            Self::A => Style::GRADE_A,
            Self::B => Style::GRADE_B,
            Self::C => Style::GRADE_C,
            Self::D => Style::GRADE_D,
            Self::F => Style::GRADE_F,
        }
    }

    /// The band's letter (`'A'`..`'F'`) for the dock row.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::A => 'A',
            Self::B => 'B',
            Self::C => 'C',
            Self::D => 'D',
            Self::F => 'F',
        }
    }

    /// Whether this is an **alarm** band — `true` for [`D`](Self::D)/[`F`](Self::F),
    /// the bands the dock hard-blinks (design #6/#16). The single predicate every
    /// grade UI keys its blink/alert off, so "which bands alarm" lives in one place.
    #[must_use]
    pub const fn is_alert(self) -> bool {
        matches!(self, Self::D | Self::F)
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants, clippy::float_cmp)]
mod tests {
    use super::{Density, GradeBand, Style};
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
    fn spell_underline_is_a_distinct_red() {
        // EDTB-6: the spelling squiggle is its own token, a red that reads as
        // "misspelling" yet is visibly distinct from the DANGER error squiggle,
        // so the editor never paints one red for two meanings.
        assert_ne!(
            Style::SPELL,
            Style::DANGER,
            "the spell squiggle must differ from the error squiggle"
        );
        let (r, g, b) = (
            Style::SPELL.r() as u16,
            Style::SPELL.g() as u16,
            Style::SPELL.b() as u16,
        );
        assert!(
            r > g && r > b,
            "the spell underline reads as red (r dominates): {r},{g},{b}"
        );
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
        assert!(Style::BODY < Style::TITLE);
        assert!(Style::TITLE < Style::HEADING);
        assert!(Style::HEADING < Style::DISPLAY);
    }

    #[test]
    fn heading_ramp_never_grows_with_depth() {
        // EDTB-7 — the markdown-preview heading ramp: H1 is the largest rung and
        // each deeper level is no larger than the one above it (a monotonic
        // non-increasing ramp), so a deeper heading never out-sizes a shallower.
        let sizes: Vec<f32> = (1..=6).map(Style::heading_size).collect();
        assert_eq!(sizes[0], Style::DISPLAY, "H1 is the display rung");
        assert_eq!(sizes[1], Style::HEADING, "H2 is the section-heading rung");
        assert_eq!(sizes[2], Style::TITLE, "H3 is the sub-heading rung");
        for w in sizes.windows(2) {
            assert!(w[0] >= w[1], "heading ramp must not grow with depth");
        }
        // Every rung is a shared type-scale token — no orphan literal size.
        for size in sizes {
            assert!(
                [Style::DISPLAY, Style::HEADING, Style::TITLE, Style::BODY].contains(&size),
                "heading size {size} is off the shared type ramp"
            );
        }
    }

    #[test]
    fn emphasis_text_is_brighter_than_body_and_opaque() {
        // EDTB-7 — the bold/heading tone reads one rung brighter than body text
        // (the honest weight cue for a font with no bold cut) and stays opaque so
        // a bold glyph never ghosts on the dark ground.
        let sum = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(
            sum(Style::TEXT_STRONG) > sum(Style::TEXT),
            "TEXT_STRONG must sit brighter than TEXT"
        );
        assert!(
            sum(Style::TEXT) > sum(Style::TEXT_DIM),
            "TEXT must sit brighter than TEXT_DIM"
        );
        assert_eq!(Style::TEXT_STRONG.a(), 0xFF, "emphasis text must be opaque");
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
        // The refactored install routes the accent through the shared derivation, so
        // the whole interactive-accent field group lands on the brand accent.
        assert_eq!(ctx.style().visuals.widgets.active.bg_fill, Style::ACCENT);
        assert_eq!(ctx.style().visuals.selection.stroke.color, Style::ACCENT);
    }

    #[test]
    fn set_accent_retints_the_live_interactive_accent_only() {
        // SETTINGS-5: a runtime accent choice re-tints the whole interactive-accent
        // field group over an installed look, and leaves the rest of the palette /
        // spacing untouched (a targeted re-tint, not a re-install).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        Style::set_accent(&ctx, Style::ACCENT_MESH);
        let s = ctx.style();
        assert_eq!(s.visuals.hyperlink_color, Style::ACCENT_MESH);
        assert_eq!(s.visuals.widgets.active.bg_fill, Style::ACCENT_MESH);
        assert_eq!(
            s.visuals.widgets.hovered.bg_stroke.color,
            Style::ACCENT_MESH
        );
        assert_eq!(s.visuals.selection.stroke.color, Style::ACCENT_MESH);
        // Untouched: the base palette + the spacing grid stay as installed.
        assert_eq!(s.visuals.panel_fill, Style::BG);
        assert_eq!(s.visuals.override_text_color, Some(Style::TEXT));
        assert_eq!(s.spacing.indent, Style::SP_M);
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

    // --- NODE-GRADE-3: the A–F grade ramp ------------------------------------

    /// How red-vs-green a colour is: positive = redder, negative = greener. This is
    /// the ramp's honest monotone axis — golds/oranges are too bright to rank by
    /// luminance, so the grades "redden" A→F, they do not simply darken.
    fn redness(c: egui::Color32) -> i32 {
        i32::from(c.r()) - i32::from(c.g())
    }

    #[test]
    fn grade_ramp_reddens_monotonically_a_to_f() {
        let ramp = [
            Style::GRADE_A,
            Style::GRADE_B,
            Style::GRADE_C,
            Style::GRADE_D,
            Style::GRADE_F,
        ];
        for pair in ramp.windows(2) {
            assert!(
                redness(pair[0]) < redness(pair[1]),
                "the grade ramp must redden strictly from A→F: {:?} !< {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn grade_bands_map_to_distinct_tokens() {
        let bands = [
            GradeBand::A,
            GradeBand::B,
            GradeBand::C,
            GradeBand::D,
            GradeBand::F,
        ];
        for (i, a) in bands.iter().enumerate() {
            for b in &bands[i + 1..] {
                assert_ne!(
                    a.color(),
                    b.color(),
                    "each grade band must map to a distinct token"
                );
            }
        }
        // The enum resolves to exactly the named ramp tokens.
        assert_eq!(GradeBand::A.color(), Style::GRADE_A);
        assert_eq!(GradeBand::F.color(), Style::GRADE_F);
    }

    #[test]
    fn grade_a_c_f_are_distinct_and_from_the_shared_palette() {
        // A/C/F are mutually distinct rungs...
        assert_ne!(Style::GRADE_A, Style::GRADE_C);
        assert_ne!(Style::GRADE_C, Style::GRADE_F);
        assert_ne!(Style::GRADE_A, Style::GRADE_F);
        // ...and every rung is an existing status/accent token, not a new hue (§4).
        assert_eq!(Style::GRADE_A, Style::OK);
        assert_eq!(Style::GRADE_B, Style::ACCENT_MESH);
        assert_eq!(Style::GRADE_C, Style::ACCENT_SYSTEM);
        assert_eq!(Style::GRADE_D, Style::WARN);
        assert_eq!(Style::GRADE_F, Style::DANGER);
    }

    #[test]
    fn grade_bands_follow_the_classic_thresholds() {
        assert_eq!(GradeBand::from_score(100.0), GradeBand::A);
        assert_eq!(GradeBand::from_score(90.0), GradeBand::A);
        assert_eq!(GradeBand::from_score(89.9), GradeBand::B);
        assert_eq!(GradeBand::from_score(80.0), GradeBand::B);
        assert_eq!(GradeBand::from_score(70.0), GradeBand::C);
        assert_eq!(GradeBand::from_score(60.0), GradeBand::D);
        assert_eq!(GradeBand::from_score(59.9), GradeBand::F);
        assert_eq!(GradeBand::from_score(0.0), GradeBand::F);
        // An unscored (NaN) node reads as the worst band, never as healthy.
        assert_eq!(GradeBand::from_score(f32::NAN), GradeBand::F);
    }

    #[test]
    fn only_d_and_f_are_alarm_bands() {
        assert!(!GradeBand::A.is_alert());
        assert!(!GradeBand::B.is_alert());
        assert!(!GradeBand::C.is_alert());
        assert!(GradeBand::D.is_alert());
        assert!(GradeBand::F.is_alert());
    }

    #[test]
    fn grade_fill_spans_the_ramp_and_reddens_as_the_score_drops() {
        // Endpoints pin to the band colours.
        assert_eq!(Style::grade_fill(100.0), Style::GRADE_A);
        assert_eq!(Style::grade_fill(0.0), Style::GRADE_F);
        // Out-of-range scores clamp to the ends rather than panic.
        assert_eq!(Style::grade_fill(250.0), Style::GRADE_A);
        assert_eq!(Style::grade_fill(-10.0), Style::GRADE_F);
        // A lower score yields a redder fill, along the same axis as the ramp.
        assert!(redness(Style::grade_fill(20.0)) > redness(Style::grade_fill(95.0)));
    }
}
