//! `Style` — the single source of look for every E12 surface (governance §4, lock 9).
//!
//! A Rust module, not a token crate: there is deliberately **no raw-literal lint
//! gate** (the §0-Simple lever), so this module *is* the discipline. Surfaces read
//! these constants and call [`Style::install`]; they never hand-roll a colour or a
//! spacing value.
//!
//! The palette is the **Construct dark** identity (the platform default). Values
//! are render-agnostic data, so they are unit-tested without a GPU.

use egui::{
    epaint::{ClippedShape, ColorMode},
    Color32, Context, Stroke,
};

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
    /// **Compact** pointer density — tighter spacing for dense/expert layouts. A
    /// user-selectable preset (not formfactor-driven); keeps the pointer 24pt hit
    /// target, only the spacing tightens.
    Compact,
    /// **Mouse** — the compact desktop metrics (laptop / windowed fallback).
    #[default]
    Mouse,
    /// **Comfortable** — roomier spacing with the finger-sized hit target. A
    /// user-selectable preset for reach/legibility without full touch metrics.
    Comfortable,
    /// **Touch** — larger hit targets and the widest spacing (tablet formfactor).
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
            // Pointer densities keep the compact 24pt control height; finger
            // densities grow to the ~44pt touch target. Density scales the spacing
            // *family* and the hit-target floor — never a component's own drawn
            // dimension (design lock #7 / UX-24).
            Self::Compact | Self::Mouse => 24.0,
            Self::Comfortable | Self::Touch => 44.0,
        }
    }

    /// The multiplier applied to the base 8px spacing grid (item spacing, button
    /// padding, indent) so a touch surface has more breathing room between targets.
    #[must_use]
    pub const fn spacing_scale(self) -> f32 {
        match self {
            Self::Compact => 0.75,
            Self::Mouse => 1.0,
            Self::Comfortable => 1.25,
            Self::Touch => 1.5,
        }
    }
}

/// The platform colour mode applied by Personalization → Theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleColorScheme {
    /// The current Construct dark palette. This is the default and preserves the
    /// status-quo colours for operators that do not opt into light mode.
    #[default]
    Dark,
    /// A Windows 2000 basic-inspired light palette: classic button-face gray,
    /// white raised surfaces, gray borders, black text, and active-title blue.
    Light,
}

impl StyleColorScheme {
    /// Visible mode order.
    pub const ALL: [Self; 2] = [Self::Dark, Self::Light];
}

/// Runtime-resolved surface/text palette for a [`StyleColorScheme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StylePalette {
    /// Top-level app and panel background.
    pub bg: Color32,
    /// Raised control/card/input surface.
    pub surface: Color32,
    /// Hovered or highlighted raised surface.
    pub surface_hi: Color32,
    /// Hairline border and separator color.
    pub border: Color32,
    /// Blank capture clear color for this mode.
    pub capture_clear: Color32,
    /// Primary readable text.
    pub text: Color32,
    /// Secondary or de-emphasized text.
    pub text_dim: Color32,
    /// Strong/emphasis text.
    pub text_strong: Color32,
}

/// The shared egui design system. All fields are `const` so they are usable in
/// `const` contexts and read directly at call sites.
pub struct Style;

impl Style {
    // ── Palette (Construct dark) ───────────────────────────────────────────────
    /// Window / app background — the deepest surface.
    pub const BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1A);
    /// Raised surface — panels, cards, inputs.
    pub const SURFACE: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x25);
    /// Hovered / highlighted surface.
    pub const SURFACE_HI: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x32);
    /// Hairline borders + separators.
    pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x3D);

    // ── Palette (Windows 2000 basic light) ──────────────────────────────────
    /// Windows 2000 classic `ButtonFace`.
    pub const WIN2000_BUTTON_FACE: Color32 = Color32::from_rgb(0xD4, 0xD0, 0xC8);
    /// Windows 2000 classic raised/light face.
    pub const WIN2000_BUTTON_HIGHLIGHT: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    /// Windows 2000 classic shadow/border.
    pub const WIN2000_BUTTON_SHADOW: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
    /// Windows 2000 classic `WindowText`.
    pub const WIN2000_WINDOW_TEXT: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
    /// A readable dim text on classic button-face gray.
    pub const WIN2000_DIM_TEXT: Color32 = Color32::from_rgb(0x40, 0x40, 0x40);
    /// Windows 2000 classic active title/highlight blue.
    pub const WIN2000_ACTIVE_TITLE: Color32 = Color32::from_rgb(0x0A, 0x24, 0x6A);
    /// Windows 2000 classic pressed button face.
    pub const WIN2000_PRESSED_FACE: Color32 = Color32::from_rgb(0xB8, 0xB4, 0xAC);

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

    // ── Overlays & capture (soft-Carbon depth, lock 2) ──────────────────────
    /// The dimming **scrim** painted under a modal / over a frozen surface — a
    /// translucent black so the layer beneath reads as pushed back without a
    /// gaussian-blur pass (lock 2: subtle-alpha dim, never a heavy wash). The one
    /// shared scrim tone every overlay dims with (the VDI reconnect overlay over a
    /// frozen desktop today), so a leaked `from_black_alpha` never re-decides "how
    /// dark is a scrim" at a call site (§4).
    pub const SCRIM: Color32 = Color32::from_black_alpha(0xB4);
    /// The blank-canvas fill of a **headless capture** (screenshot rasterizer): a
    /// neutral near-black held **strictly darker than every real surface tone** the
    /// shell paints, so a genuinely blank capture is obvious in the PNG itself, not
    /// only via a pixel scan. Its own token — not [`BG`](Self::BG) — precisely
    /// *because* it must not collide with a real surface (asserted by the tests).
    pub const CAPTURE_CLEAR: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);

    /// Primary text.
    pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xEC);
    /// Secondary / dimmed text.
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0xA6);
    /// **Emphasis** text — one rung brighter than [`TEXT`](Self::TEXT) (Carbon
    /// white). The shared font installer embeds Inter for proportional UI and
    /// Intel One Mono for monospace surfaces; the honest token cue for emphasis
    /// on the dark ground is this brighter tone, the mirror of
    /// [`TEXT_DIM`](Self::TEXT_DIM)'s dimmer one. Markdown preview (EDTB-7)
    /// paints bold spans + heading titles with it.
    pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);

    /// Interactive / brand accent (Construct azure).
    pub const ACCENT: Color32 = Color32::from_rgb(0x5B, 0x8C, 0xFF);
    /// Accent, hovered.
    pub const ACCENT_HI: Color32 = Color32::from_rgb(0x7A, 0xA2, 0xFF);

    /// Status — danger / error.
    pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x5C, 0x57);
    /// Status — warning.
    pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB4, 0x54);
    /// Status — success / ok.
    pub const OK: Color32 = Color32::from_rgb(0x4F, 0xD0, 0x8A);

    // ── Carbon semantic status tokens ──────────────────────────────────────
    // NOTIF-1 / Q25-Q28: one shared severity language for pips, Chat alert
    // cards, and any future segment rollup. These are semantic aliases over the
    // existing Carbon-compatible status palette, so downstream surfaces stop
    // re-deciding that "red means critical" with local DANGER/WARN/ACCENT calls.
    /// Carbon **support-error** — red alert / action-needed severity.
    pub const SUPPORT_ERROR: Color32 = Self::DANGER;
    /// Carbon **support-warning** — amber warning severity.
    pub const SUPPORT_WARNING: Color32 = Self::WARN;
    /// Carbon **support-success** — green success / resolved severity.
    pub const SUPPORT_SUCCESS: Color32 = Self::OK;
    /// Carbon **support-info** — blue informational severity.
    pub const SUPPORT_INFO: Color32 = Self::ACCENT;
    /// Editorial — a **spelling** miss: the red wavy underline the editor draws
    /// under a misspelled word (EDTB-6, `mde-editor-egui`). Its own token — a
    /// deeper, more saturated Carbon red-60 — so a spell squiggle reads distinct
    /// from a [`DANGER`](Self::DANGER) *error* underline (the LSP diagnostics
    /// squiggle), never the same red for two different meanings.
    pub const SPELL: Color32 = Color32::from_rgb(0xDA, 0x1E, 0x28);

    // ── Categorical accents (picker groups · explorer categories) ───────────
    // Distinct categorical hues that key a group's / category's
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
    /// Categorical accent — **Web** (Chrome primary blue).
    pub const ACCENT_WEB: Color32 = Color32::from_rgb(0x0B, 0x57, 0xD0);
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

    // ── Corner-radius tiers (applied by surfaces at draw time as raw data, so the
    //    harness build stays free of egui's version-sensitive corner-radius type) ──
    /// Tight radius — buttons, chips, taskbar/cell inner fills.
    pub const RADIUS_S: f32 = 4.0;
    /// Mid radius — cards, menus, popovers (the historical default).
    pub const RADIUS_M: f32 = 6.0;
    /// Large radius — windows, sheets, dialogs, the lock curtain.
    pub const RADIUS_L: f32 = 8.0;
    /// Back-compat alias for the mid tier — the ~130 pre-tier call sites read this.
    pub const RADIUS: f32 = Self::RADIUS_M;

    // ── Type scale (point sizes) ────────────────────────────────────────────
    /// Small / caption text.
    pub const SMALL: f32 = 10.0;
    /// Body text.
    pub const BODY: f32 = 12.0;
    /// Sub-heading (Carbon productive-heading-04) — between [`BODY`](Self::BODY)
    /// and [`HEADING`](Self::HEADING); the markdown-preview H3 rung (EDTB-7).
    pub const TITLE: f32 = 16.0;
    /// Section heading.
    pub const HEADING: f32 = 20.0;
    /// Display heading (Carbon productive-heading-06) — the largest type rung,
    /// the markdown-preview H1 title size (EDTB-7).
    pub const DISPLAY: f32 = 26.0;

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
        Self::install_color_scheme_with_density(ctx, StyleColorScheme::Dark, density);
    }

    /// Install the shared look at an explicit colour mode and density.
    pub fn install_color_scheme_with_density(
        ctx: &Context,
        scheme: StyleColorScheme,
        density: Density,
    ) {
        // Inter is the proportional platform face; Intel One Mono stays reserved
        // for fixed-width surfaces that require monospace glyphs.
        crate::fonts::install(ctx);

        let v = Self::visuals_for(scheme, Self::ACCENT, Self::ACCENT_HI);

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
        ctx.data_mut(|d| d.insert_temp(Self::color_scheme_id(), scheme));
    }

    /// The current colour mode installed on `ctx`.
    #[must_use]
    pub fn color_scheme(ctx: &Context) -> StyleColorScheme {
        ctx.data(|d| {
            d.get_temp::<StyleColorScheme>(Self::color_scheme_id())
                .unwrap_or_default()
        })
    }

    /// The runtime palette installed on `ctx`.
    #[must_use]
    pub fn current_palette(ctx: &Context) -> StylePalette {
        Self::palette_for(Self::color_scheme(ctx))
    }

    /// The runtime palette for `scheme`.
    #[must_use]
    pub const fn palette_for(scheme: StyleColorScheme) -> StylePalette {
        match scheme {
            StyleColorScheme::Dark => StylePalette {
                bg: Self::BG,
                surface: Self::SURFACE,
                surface_hi: Self::SURFACE_HI,
                border: Self::BORDER,
                capture_clear: Self::CAPTURE_CLEAR,
                text: Self::TEXT,
                text_dim: Self::TEXT_DIM,
                text_strong: Self::TEXT_STRONG,
            },
            StyleColorScheme::Light => StylePalette {
                bg: Self::WIN2000_BUTTON_FACE,
                surface: Self::WIN2000_BUTTON_FACE,
                surface_hi: Self::WIN2000_BUTTON_HIGHLIGHT,
                border: Self::WIN2000_BUTTON_SHADOW,
                capture_clear: Self::WIN2000_BUTTON_FACE,
                text: Self::WIN2000_WINDOW_TEXT,
                text_dim: Self::WIN2000_DIM_TEXT,
                text_strong: Self::WIN2000_WINDOW_TEXT,
            },
        }
    }

    /// Resolve one of the static [`Style`] colour tokens into the live colour mode.
    /// Non-token colours pass through untouched.
    #[must_use]
    pub fn resolve_color(ctx: &Context, color: Color32) -> Color32 {
        Self::resolve_color_for_scheme(Self::color_scheme(ctx), color)
    }

    /// Resolve one of the static [`Style`] colour tokens into `scheme`.
    #[must_use]
    pub fn resolve_color_for_scheme(scheme: StyleColorScheme, color: Color32) -> Color32 {
        if scheme == StyleColorScheme::Dark {
            return color;
        }
        let p = Self::palette_for(scheme);
        match color {
            Self::BG => p.bg,
            Self::SURFACE => p.surface,
            Self::SURFACE_HI => p.surface_hi,
            Self::BORDER => p.border,
            Self::CAPTURE_CLEAR => p.capture_clear,
            Self::TEXT => p.text,
            Self::TEXT_DIM => p.text_dim,
            Self::TEXT_STRONG => p.text_strong,
            Self::ACCENT => Self::WIN2000_ACTIVE_TITLE,
            Self::ACCENT_HI => Self::WIN2000_ACTIVE_TITLE,
            _ => color,
        }
    }

    /// Apply `scheme` and `accent` to the live egui visuals without touching density,
    /// spacing, text scale, or animation cadence.
    pub fn set_color_scheme_and_accent(ctx: &Context, scheme: StyleColorScheme, accent: Color32) {
        ctx.style_mut(|s| {
            s.visuals = Self::visuals_for(scheme, accent, accent);
        });
        ctx.data_mut(|d| d.insert_temp(Self::color_scheme_id(), scheme));
    }

    fn color_scheme_id() -> egui::Id {
        egui::Id::new("mde-egui-style-color-scheme")
    }

    fn visuals_for(scheme: StyleColorScheme, accent: Color32, accent_hi: Color32) -> egui::Visuals {
        let p = Self::palette_for(scheme);
        let mut v = match scheme {
            StyleColorScheme::Dark => egui::Visuals::dark(),
            StyleColorScheme::Light => egui::Visuals::light(),
        };

        v.panel_fill = p.bg;
        v.window_fill = p.surface;
        v.extreme_bg_color = p.bg;
        v.faint_bg_color = p.surface;
        v.override_text_color = Some(p.text);
        v.window_stroke = Stroke::new(1.0, p.border);

        let border = Stroke::new(1.0, p.border);
        let text = Stroke::new(1.0, p.text);
        let text_dim = Stroke::new(1.0, p.text_dim);

        // Non-interactive chrome (labels, separators).
        v.widgets.noninteractive.bg_fill = p.bg;
        v.widgets.noninteractive.weak_bg_fill = p.bg;
        v.widgets.noninteractive.bg_stroke = border;
        v.widgets.noninteractive.fg_stroke = text_dim;

        // Resting interactive widgets.
        v.widgets.inactive.bg_fill = p.surface;
        v.widgets.inactive.weak_bg_fill = p.surface;
        v.widgets.inactive.bg_stroke = border;
        v.widgets.inactive.fg_stroke = text;

        // Hover.
        v.widgets.hovered.bg_fill = p.surface_hi;
        v.widgets.hovered.weak_bg_fill = p.surface_hi;
        v.widgets.hovered.fg_stroke = text;

        // Pressed / active. egui's `.strong()` text colour is hardwired to this
        // stroke. Dark mode keeps the historical bright-on-dark pressed style;
        // light mode keeps strong text black and uses a classic gray pressed face.
        v.widgets.active.fg_stroke = Stroke::new(1.0, p.text_strong);

        // The interactive **accent** — the ONE re-tintable field group: the
        // hyperlink, the text-selection wash + ring, the hover ring, and the
        // pressed fill + ring.
        Self::accent_visuals_for_scheme(&mut v, scheme, accent, accent_hi);
        v
    }

    /// Apply the interactive-accent derivation to `v` for one colour scheme.
    ///
    /// This field group is what a runtime accent choice re-tints (SETTINGS-5): the
    /// hyperlink colour, the text-selection wash + ring, the hover ring, and the
    /// pressed/active fill + ring. Factored so install and runtime picks share one
    /// derivation and can never fork the look (§4/§6).
    fn accent_visuals_for_scheme(
        v: &mut egui::Visuals,
        scheme: StyleColorScheme,
        accent: Color32,
        accent_hi: Color32,
    ) {
        let accent = Self::accent_for_scheme(scheme, accent);
        let accent_hi = Self::accent_for_scheme(scheme, accent_hi);
        v.hyperlink_color = accent;
        v.selection.bg_fill = accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, accent);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
        // The pressed fill is the accent DARKENED toward BG so the bright active
        // label (`TEXT_STRONG`, set in `install_with_density`) stays WCAG-legible on
        // it, while a bright accent ring keeps the pressed state unmistakably
        // accent-coloured. Opaque (blend, not `gamma_multiply`, which would fade the
        // alpha into a translucent wash like `selection.bg_fill` above).
        let pressed = Self::pressed_fill_for_scheme(scheme, accent);
        v.widgets.active.bg_fill = pressed;
        v.widgets.active.weak_bg_fill = pressed;
        v.widgets.active.bg_stroke = Stroke::new(1.0, accent_hi);
    }

    /// The visible accent for `scheme`. The default brand accent becomes the classic
    /// Windows active-title blue in light mode; explicit user accent picks remain
    /// their chosen hue.
    #[must_use]
    pub fn accent_for_scheme(scheme: StyleColorScheme, accent: Color32) -> Color32 {
        if scheme == StyleColorScheme::Light && accent == Self::ACCENT {
            Self::WIN2000_ACTIVE_TITLE
        } else {
            accent
        }
    }

    /// The pressed/active **fill** for an accent: the accent darkened toward
    /// [`BG`](Self::BG) so the bright pressed label ([`TEXT_STRONG`](Self::TEXT_STRONG),
    /// which is also egui's `strong_text_color`) stays WCAG AA legible on it for every
    /// selectable accent. The canonical derivation, so a caller (or a test) never
    /// re-hardcodes the darken factor.
    #[must_use]
    pub fn pressed_fill(accent: Color32) -> Color32 {
        Self::blend(accent, Self::BG, Self::PRESSED_FILL_DARKEN)
    }

    /// The pressed/active fill for `scheme`.
    #[must_use]
    pub fn pressed_fill_for_scheme(scheme: StyleColorScheme, accent: Color32) -> Color32 {
        match scheme {
            StyleColorScheme::Dark => Self::pressed_fill(accent),
            StyleColorScheme::Light => Self::WIN2000_PRESSED_FACE,
        }
    }

    /// Fraction the pressed/active fill is darkened toward [`BG`](Self::BG) from the
    /// live accent. Chosen so the bright pressed label clears WCAG AA body contrast on
    /// EVERY selectable accent (verified by `pressed_accent_text_stays_wcag_legible`).
    const PRESSED_FILL_DARKEN: f32 = 0.5;

    /// Opaque linear blend of two colours: `t == 0` is `a`, `t == 1` is `b`. Keeps
    /// full alpha (unlike [`Color32::gamma_multiply`], which scales alpha too).
    #[must_use]
    fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
        let mix = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
        Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
    }

    /// Re-tint the live interactive **accent** on `ctx` to `accent` (SETTINGS-5 — the
    /// Personalization → Theme accent choice). Re-applies ONLY the accent-derived
    /// visual fields (via [`accent_visuals_for_scheme`](Self::accent_visuals_for_scheme)) over the
    /// already-installed look, so the palette / density / type scale are untouched.
    /// The shell re-applies this each frame from its Settings poll, so a chosen accent
    /// survives a formfactor [`install_with_density`](Self::install_with_density)
    /// re-install. A user pick has no separate "hi" token, so the chosen accent doubles
    /// as the pressed-ring highlight.
    pub fn set_accent(ctx: &Context, accent: Color32) {
        let scheme = Self::color_scheme(ctx);
        ctx.style_mut(|s| Self::accent_visuals_for_scheme(&mut s.visuals, scheme, accent, accent));
    }

    /// Remap exact static `Style::*` token colours in paint output to `scheme`.
    ///
    /// Many shell surfaces custom-paint shapes and YAMIS glyphs with static tokens.
    /// Installing `egui::Visuals` cannot recolour those already-built primitives, so
    /// the DRM runner applies this before tessellation. Exact matching keeps status,
    /// provider, media, and arbitrary content colours intact.
    pub fn remap_clipped_shapes_for_color_scheme(
        shapes: &mut [ClippedShape],
        scheme: StyleColorScheme,
    ) {
        if scheme == StyleColorScheme::Dark {
            return;
        }
        for clipped in shapes {
            Self::remap_shape_for_color_scheme(&mut clipped.shape, scheme);
        }
    }

    fn remap_shape_for_color_scheme(shape: &mut egui::Shape, scheme: StyleColorScheme) {
        match shape {
            egui::Shape::Noop | egui::Shape::Callback(_) => {}
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    Self::remap_shape_for_color_scheme(shape, scheme);
                }
            }
            egui::Shape::Circle(circle) => {
                circle.fill = Self::resolve_color_for_scheme(scheme, circle.fill);
                circle.stroke.color = Self::resolve_color_for_scheme(scheme, circle.stroke.color);
            }
            egui::Shape::Ellipse(ellipse) => {
                ellipse.fill = Self::resolve_color_for_scheme(scheme, ellipse.fill);
                ellipse.stroke.color = Self::resolve_color_for_scheme(scheme, ellipse.stroke.color);
            }
            egui::Shape::LineSegment { stroke, .. } => {
                stroke.color = Self::resolve_color_for_scheme(scheme, stroke.color);
            }
            egui::Shape::Path(path) => {
                path.fill = Self::resolve_color_for_scheme(scheme, path.fill);
                Self::remap_path_stroke_for_color_scheme(&mut path.stroke, scheme);
            }
            egui::Shape::Rect(rect) => {
                rect.fill = Self::resolve_color_for_scheme(scheme, rect.fill);
                rect.stroke.color = Self::resolve_color_for_scheme(scheme, rect.stroke.color);
            }
            egui::Shape::Text(text) => {
                text.fallback_color = Self::resolve_color_for_scheme(scheme, text.fallback_color);
                if let Some(color) = &mut text.override_text_color {
                    *color = Self::resolve_color_for_scheme(scheme, *color);
                }
                text.underline.color = Self::resolve_color_for_scheme(scheme, text.underline.color);
                let galley = std::sync::Arc::make_mut(&mut text.galley);
                let job = std::sync::Arc::make_mut(&mut galley.job);
                for section in &mut job.sections {
                    section.format.color =
                        Self::resolve_color_for_scheme(scheme, section.format.color);
                    section.format.background =
                        Self::resolve_color_for_scheme(scheme, section.format.background);
                    section.format.underline.color =
                        Self::resolve_color_for_scheme(scheme, section.format.underline.color);
                    section.format.strikethrough.color =
                        Self::resolve_color_for_scheme(scheme, section.format.strikethrough.color);
                }
                for row in &mut galley.rows {
                    for vertex in &mut row.visuals.mesh.vertices {
                        vertex.color = Self::resolve_color_for_scheme(scheme, vertex.color);
                    }
                }
            }
            egui::Shape::Mesh(mesh) => {
                let mesh = std::sync::Arc::make_mut(mesh);
                for vertex in &mut mesh.vertices {
                    vertex.color = Self::resolve_color_for_scheme(scheme, vertex.color);
                }
            }
            egui::Shape::QuadraticBezier(bezier) => {
                bezier.fill = Self::resolve_color_for_scheme(scheme, bezier.fill);
                Self::remap_path_stroke_for_color_scheme(&mut bezier.stroke, scheme);
            }
            egui::Shape::CubicBezier(bezier) => {
                bezier.fill = Self::resolve_color_for_scheme(scheme, bezier.fill);
                Self::remap_path_stroke_for_color_scheme(&mut bezier.stroke, scheme);
            }
        }
    }

    fn remap_path_stroke_for_color_scheme(
        stroke: &mut egui::epaint::PathStroke,
        scheme: StyleColorScheme,
    ) {
        if let ColorMode::Solid(color) = &mut stroke.color {
            *color = Self::resolve_color_for_scheme(scheme, *color);
        }
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

    /// The translucent **accent selection wash** — the fill of a drag-select /
    /// region-capture marquee drawn over content (the browser capture-region
    /// rectangle). The brand [`ACCENT`](Self::ACCENT) at a light alpha, paired with
    /// the marquee's 1 px `ACCENT` ring so a selection reads as one accent
    /// affordance. Derived from the accent (not a bespoke hue) so a theme re-tint
    /// carries the wash with it — before this it was an off-palette IBM-blue literal
    /// minted at the call site (§4).
    #[must_use]
    pub fn selection_wash() -> Color32 {
        Self::ACCENT.gamma_multiply(0.16)
    }

    /// The translucent **visual-bell flash** tint — the brief attention wash a
    /// terminal paints over its pane when the shell rings the bell with audio
    /// muted (TERM-12). A premultiplied **white** at the supplied `alpha`: the
    /// pane momentarily lightens then decays back as the surface eases `alpha`
    /// down each frame (`0` fully transparent, `255` solid white). The flash is
    /// the tonal *opposite* of the [`SCRIM`](Self::SCRIM) dim — an attention
    /// **lift**, not a push-back — so it earns its own token rather than
    /// re-deciding "the bell is white" at the call site (§4).
    #[must_use]
    pub fn bell_flash(alpha: u8) -> Color32 {
        Color32::from_white_alpha(alpha)
    }

    // ── Colour algebra (pixel-DATA helpers, sibling to `blend`) ─────────────
    // The per-pixel colour math the shell's software surfaces need but which is
    // *not* a design token: routed through the shared kit so no surface crate
    // mints a raw `Color32` for pixel work (§4 / CRAFT §1 — add the primitive
    // here with a test, never approximate it locally).

    /// Fold a colour toward black by scaling its RGB channels by `k` ∈ `[0, 1]`
    /// **while forcing full opacity** — the lock curtain's idle dim, which must
    /// stay opaque (a dimmed curtain must never become a window onto the desktop).
    /// `k = 1.0` returns the colour opaque; `k = 0.0` returns opaque black.
    #[must_use]
    pub fn scale_rgb_opaque(c: Color32, k: f32) -> Color32 {
        let s = |v: u8| scale_channel(v, k);
        Color32::from_rgba_premultiplied(s(c.r()), s(c.g()), s(c.b()), u8::MAX)
    }

    /// Luminance-key a colour's alpha: keep the RGB, set alpha to the brightest
    /// channel (unmultiplied), so a sprite's dark surround fades to transparent and
    /// its bright core stays solid — the splash head-dot glow key.
    #[must_use]
    pub fn key_alpha_to_luma(c: Color32) -> Color32 {
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.r().max(c.g()).max(c.b()))
    }
}

/// Scale one 8-bit channel by `k`, clamped to `0..=255` (`k` is clamped to
/// `[0, 1]` first) — the byte kernel behind [`Style::scale_rgb_opaque`].
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "k is clamped to [0,1], so v·k stays in 0..=255"
)]
fn scale_channel(v: u8, k: f32) -> u8 {
    (f32::from(v) * k.clamp(0.0, 1.0)).round() as u8
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

/// A soft-shadow token (raw data; a surface builds `epaint::Shadow` from it at
/// draw time, keeping this module free of egui's shadow type). The umbra is
/// **always** a translucent black (`a() < 255`): depth is alpha + a soft blur,
/// never an opaque fill and never a true gaussian-blur *pass* over the content
/// behind (design lock #2 — "layered soft shadows … no blur pass").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowToken {
    /// `(x, y)` cast offset in logical px.
    pub offset: [f32; 2],
    /// Blur radius in logical px (`epaint::Shadow::blur`).
    pub blur: f32,
    /// Spread in logical px (`epaint::Shadow::spread`).
    pub spread: f32,
    /// Umbra colour — a translucent black; the invariant is `a() < 255`.
    pub umbra: Color32,
}

/// The elevation ladder — how far a surface sits off the page. Each tier maps to
/// one [`ShadowToken`] via [`shadow`](Self::shadow); [`Flat`](Self::Flat) casts
/// none. Higher tiers cast a larger, softer, slightly deeper shadow, but every
/// umbra stays translucent. This is the single source of "how deep is a card /
/// menu / dialog", so no surface hand-rolls a `Shadow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// On the page — no shadow (inline chrome, list rows).
    Flat,
    /// A card / cell lifted off its surface.
    Raised,
    /// A floating overlay — menu, popover, tooltip, the taskbar Start grid.
    Overlay,
    /// A modal sheet / dialog / lock curtain — the deepest tier.
    Modal,
}

impl Elevation {
    /// The soft-shadow token for this tier (offset/blur grow with elevation; the
    /// umbra stays translucent at every tier).
    #[must_use]
    pub const fn shadow(self) -> ShadowToken {
        match self {
            Self::Flat => ShadowToken {
                offset: [0.0, 0.0],
                blur: 0.0,
                spread: 0.0,
                umbra: Color32::from_black_alpha(0),
            },
            Self::Raised => ShadowToken {
                offset: [0.0, 1.0],
                blur: 2.0,
                spread: 0.0,
                umbra: Color32::from_black_alpha(0x30),
            },
            Self::Overlay => ShadowToken {
                offset: [0.0, 4.0],
                blur: 12.0,
                spread: 0.0,
                umbra: Color32::from_black_alpha(0x50),
            },
            Self::Modal => ShadowToken {
                offset: [0.0, 8.0],
                blur: 24.0,
                spread: 0.0,
                umbra: Color32::from_black_alpha(0x70),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants, clippy::float_cmp)]
mod tests {
    use super::{Density, Elevation, GradeBand, Style, StyleColorScheme};
    use crate::formfactor::Formfactor;

    /// WCAG 2.1 **relative luminance** of an sRGB colour (`0.0..=1.0`; alpha ignored).
    /// Each 8-bit channel is normalized, linearized through the sRGB EOTF, then weighted
    /// `0.2126 R + 0.7152 G + 0.0722 B`. Pure data math — no GPU — matching this module's
    /// render-agnostic contract (the tokens are data, testable without a render pass).
    fn relative_luminance(c: egui::Color32) -> f32 {
        let lin = |ch: u8| {
            let s = f32::from(ch) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// The WCAG 2.1 **contrast ratio** between two sRGB colours: `(L_light + 0.05) /
    /// (L_dark + 0.05)`, in `1.0..=21.0`. Symmetric — the brighter colour is always the
    /// numerator, so argument order does not matter.
    fn wcag_contrast_ratio(a: egui::Color32, b: egui::Color32) -> f32 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn strong_text_stays_bright() {
        // egui hardcodes `Visuals::strong_text_color()` to `widgets.active.fg_stroke`.
        // Every `.strong()` label across the shell renders in that colour, so it MUST
        // be bright on the dark canvas — a regression to a near-BG value paints strong
        // text black and it reads as "disabled". Pin it well clear of the AA floor.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let strong = ctx.style().visuals.strong_text_color();
        assert_eq!(
            strong,
            Style::TEXT_STRONG,
            "strong_text_color must be the bright label token, not a dark/pressed colour"
        );
        let ratio = wcag_contrast_ratio(strong, Style::BG);
        assert!(
            ratio >= 7.0,
            "strong text over BG is only {ratio:.2}:1 — strong/emphasised text must stay \
             high-contrast (a near-BG value here blackens every .strong() label)"
        );
    }

    #[test]
    fn pressed_accent_text_stays_wcag_legible() {
        // The pressed/active widget paints its label in Style::TEXT_STRONG
        // (`install_with_density`) over a DARKENED accent fill
        // (`blend(accent, BG, PRESSED_FILL_DARKEN)`). A button label renders at
        // Style::BODY (12 pt) — below WCAG's "large text" cut (18 pt, or 14 pt bold) —
        // so the applicable AA legibility floor is the body-text 4.5:1, NOT the 3:1
        // large-text / UI-component relaxation. This guard stops a future accent (or a
        // change to the darken factor) from dropping the bright pressed label below
        // readable contrast on its own fill.
        const AA_BODY: f32 = 4.5;

        // Sanity anchors — the two known-good ratios named in the platform review.
        let text_bg = wcag_contrast_ratio(Style::TEXT, Style::BG);
        assert!(
            (text_bg - 14.52).abs() < 0.1,
            "TEXT/BG contrast drifted from the known 14.52:1 anchor: {text_bg:.2}"
        );
        let accent_bg = wcag_contrast_ratio(Style::ACCENT, Style::BG);
        assert!(
            (accent_bg - 5.71).abs() < 0.1,
            "ACCENT/BG contrast drifted from the known 5.71:1 anchor: {accent_bg:.2}"
        );

        // Every accent the shell's Personalization → Theme picker (SETTINGS-5,
        // `AccentChoice`) can select IS one of these shared tokens — Brand→ACCENT,
        // Cyan→ACCENT_COMMS, Purple→ACCENT_WORKLOADS, Teal→ACCENT_TERMINALS,
        // Green→ACCENT_MESH, Gold→ACCENT_SYSTEM, Magenta→ACCENT_MEDIA — so guarding the
        // tokens here guards every selectable pressed accent, with no mde-egui→shell dep.
        let accents = [
            ("ACCENT (Brand)", Style::ACCENT),
            ("ACCENT_HI", Style::ACCENT_HI),
            ("ACCENT_COMMS (Cyan)", Style::ACCENT_COMMS),
            ("ACCENT_WORKLOADS (Purple)", Style::ACCENT_WORKLOADS),
            ("ACCENT_TERMINALS (Teal)", Style::ACCENT_TERMINALS),
            ("ACCENT_WEB (Chrome blue)", Style::ACCENT_WEB),
            ("ACCENT_MESH (Green)", Style::ACCENT_MESH),
            ("ACCENT_SYSTEM (Gold)", Style::ACCENT_SYSTEM),
            ("ACCENT_MEDIA (Magenta)", Style::ACCENT_MEDIA),
        ];
        for (name, accent) in accents {
            // The pressed label is TEXT_STRONG over the darkened accent fill.
            let fill = Style::pressed_fill(accent);
            let ratio = wcag_contrast_ratio(Style::TEXT_STRONG, fill);
            assert!(
                ratio >= AA_BODY,
                "pressed TEXT_STRONG label over the darkened {name} fill is only \
                 {ratio:.2}:1 — below the WCAG AA body-text floor of {AA_BODY}:1; \
                 pressed-button labels would be unreadable"
            );
        }
    }

    #[test]
    fn spacing_follows_the_8px_grid() {
        for s in [Style::SP_S, Style::SP_M, Style::SP_L, Style::SP_XL] {
            assert_eq!(s % 8.0, 0.0, "{s} is off the 8px grid");
        }
        // XS is the deliberate half-step.
        assert_eq!(Style::SP_XS, Style::SP_S / 2.0);
    }

    #[test]
    fn radius_tiers_ascend_and_default_is_the_mid_tier() {
        // Strictly ascending, each on the 2px sub-grid, mid == the back-compat alias.
        assert!(
            Style::RADIUS_S < Style::RADIUS_M && Style::RADIUS_M < Style::RADIUS_L,
            "radius tiers must strictly ascend: {} < {} < {}",
            Style::RADIUS_S,
            Style::RADIUS_M,
            Style::RADIUS_L,
        );
        for r in [Style::RADIUS_S, Style::RADIUS_M, Style::RADIUS_L] {
            assert_eq!(r % 2.0, 0.0, "{r} is off the 2px sub-grid");
        }
        assert_eq!(
            Style::RADIUS,
            Style::RADIUS_M,
            "RADIUS must alias the mid tier so pre-tier call sites are unchanged"
        );
    }

    #[test]
    fn elevation_ladder_is_soft_and_ascends() {
        use Elevation::{Flat, Modal, Overlay, Raised};
        // Flat casts nothing.
        assert_eq!(Flat.shadow().umbra.a(), 0, "Flat must cast no shadow");
        assert_eq!(Flat.shadow().blur, 0.0);
        // The three real tiers grow in offset + blur and deepen in umbra …
        let tiers = [Raised, Overlay, Modal];
        for w in tiers.windows(2) {
            let (lo, hi) = (w[0].shadow(), w[1].shadow());
            assert!(
                hi.offset[1] > lo.offset[1],
                "shadow y-offset must grow with elevation"
            );
            assert!(hi.blur > lo.blur, "blur must grow with elevation");
            assert!(
                hi.umbra.a() > lo.umbra.a(),
                "umbra must deepen with elevation"
            );
        }
        // … but the umbra is ALWAYS translucent — depth is alpha, never opaque (lock #2).
        for e in [Raised, Overlay, Modal] {
            let a = e.shadow().umbra.a();
            assert!(
                a > 0 && a < 255,
                "{e:?} umbra alpha {a} must be a soft (0,255)"
            );
        }
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
            u16::from(Style::SPELL.r()),
            u16::from(Style::SPELL.g()),
            u16::from(Style::SPELL.b()),
        );
        assert!(
            r > g && r > b,
            "the spell underline reads as red (r dominates): {r},{g},{b}"
        );
    }

    #[test]
    fn categorical_accents_are_a_distinct_palette() {
        // PICKER-2 / EXPLORER-15 O8: the shared picker-group / explorer-category
        // accents must be mutually distinguishable — one colour language —
        // and each set apart from the single interactive brand accent so a category
        // tint never reads as an interaction affordance.
        let cats = [
            Style::ACCENT_COMMS,
            Style::ACCENT_WORKLOADS,
            Style::ACCENT_TERMINALS,
            Style::ACCENT_WEB,
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
    fn scrim_is_a_translucent_black() {
        // The shared overlay dim (lock 2): a black at partial alpha — dark enough
        // to push a layer back, never fully opaque (it must still reveal the
        // dimmed surface beneath).
        assert_eq!(
            (Style::SCRIM.r(), Style::SCRIM.g(), Style::SCRIM.b()),
            (0, 0, 0),
            "the scrim is a black wash"
        );
        assert!(
            Style::SCRIM.a() > 0 && Style::SCRIM.a() < 255,
            "the scrim is translucent, not opaque: a={}",
            Style::SCRIM.a()
        );
    }

    #[test]
    fn capture_clear_is_darker_than_every_surface() {
        // The headless-capture blank fill must sit strictly below every real
        // surface tone so a blank capture reads as blank in the PNG — and never
        // aliases onto an actual surface token.
        let luma = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        for surface in [Style::BG, Style::SURFACE, Style::SURFACE_HI, Style::BORDER] {
            assert!(
                luma(Style::CAPTURE_CLEAR) < luma(surface),
                "CAPTURE_CLEAR must be darker than every surface tone"
            );
            assert_ne!(
                Style::CAPTURE_CLEAR,
                surface,
                "CAPTURE_CLEAR must not collide with a real surface token"
            );
        }
    }

    #[test]
    fn selection_wash_is_a_translucent_accent() {
        // The drag-select marquee fill: the brand accent at a light alpha, so it
        // shares the accent hue with the marquee's ring and re-tints with the theme.
        let wash = Style::selection_wash();
        assert!(
            wash.a() > 0 && wash.a() < 255,
            "the selection wash is a light translucent fill: a={}",
            wash.a()
        );
        assert!(
            wash.b() > wash.r(),
            "the wash keeps the accent's blue-forward hue"
        );
    }

    #[test]
    fn bell_flash_is_a_translucent_white_that_scales_with_alpha() {
        // TERM-12: the visual-bell flash is a premultiplied white wash whose
        // alpha the surface eases down each frame — fully transparent at 0, solid
        // white at full, and a tonal lift (not the SCRIM's black push-back) in
        // between.
        assert_eq!(
            Style::bell_flash(0).a(),
            0,
            "a zero-intensity flash is fully transparent"
        );
        assert_eq!(
            Style::bell_flash(255),
            egui::Color32::WHITE,
            "a full flash is solid white"
        );

        let mid = Style::bell_flash(90);
        assert_eq!(mid.a(), 90, "the flash alpha passes through");
        assert!(
            mid.r() == mid.g() && mid.g() == mid.b(),
            "the flash is an achromatic white: {},{},{}",
            mid.r(),
            mid.g(),
            mid.b()
        );
        assert!(mid.r() > 0, "a non-zero flash tints the pane");
        assert!(
            Style::bell_flash(200).r() > mid.r(),
            "a stronger flash is a brighter lift"
        );
        // A lift where the scrim dims — the opposite tonal intent.
        assert_ne!(mid, Style::SCRIM, "the bell flash is not the scrim dim");
    }

    #[test]
    fn scale_rgb_opaque_dims_rgb_but_stays_opaque() {
        // The curtain idle dim: RGB folds toward black, alpha never drops (a dimmed
        // curtain must stay a solid sheet, not a window).
        let full = Style::scale_rgb_opaque(egui::Color32::WHITE, 1.0);
        assert_eq!(
            full,
            egui::Color32::WHITE,
            "k=1 leaves the colour untouched"
        );

        let half = Style::scale_rgb_opaque(egui::Color32::WHITE, 0.5);
        assert_eq!(half.a(), 255, "the dim stays fully opaque");
        assert!(
            half.r() < 255 && half.r() > 0,
            "k=0.5 dims the channels: r={}",
            half.r()
        );

        let dark = Style::scale_rgb_opaque(egui::Color32::WHITE, 0.0);
        assert_eq!(
            dark,
            egui::Color32::from_rgb(0, 0, 0),
            "k=0 folds to opaque black"
        );
    }

    #[test]
    fn key_alpha_to_luma_keys_alpha_to_the_brightest_channel() {
        // The splash head-dot key: alpha follows the brightest channel, so a bright
        // core stays solid and a dark surround fades out.
        let bright = Style::key_alpha_to_luma(egui::Color32::from_rgb(30, 255, 60));
        assert_eq!(bright.a(), 255, "a fully-bright channel keys to opaque");
        assert_eq!(
            (bright.r(), bright.g(), bright.b()),
            (30, 255, 60),
            "an opaque result preserves the RGB unchanged"
        );

        let mid = Style::key_alpha_to_luma(egui::Color32::from_rgb(100, 50, 0));
        assert_eq!(mid.a(), 100, "alpha keys to the brightest channel");

        let dark = Style::key_alpha_to_luma(egui::Color32::from_rgb(0, 0, 0));
        assert_eq!(dark.a(), 0, "a black surround keys fully transparent");
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
        // the whole interactive-accent field group lands on the (darkened) brand accent,
        // with a bright pressed label — the same colour egui reuses for strong text.
        assert_eq!(
            ctx.style().visuals.widgets.active.bg_fill,
            Style::pressed_fill(Style::ACCENT)
        );
        assert_eq!(
            ctx.style().visuals.widgets.active.fg_stroke.color,
            Style::TEXT_STRONG
        );
        assert_eq!(ctx.style().visuals.selection.stroke.color, Style::ACCENT);
    }

    #[test]
    fn light_install_uses_windows_2000_basic_palette() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::Light, Density::Mouse);
        let visuals = &ctx.style().visuals;
        let p = Style::palette_for(StyleColorScheme::Light);

        assert_eq!(Style::color_scheme(&ctx), StyleColorScheme::Light);
        assert_eq!(visuals.panel_fill, Style::WIN2000_BUTTON_FACE);
        assert_eq!(visuals.window_fill, p.surface);
        assert_eq!(visuals.extreme_bg_color, p.bg);
        assert_eq!(visuals.window_stroke.color, Style::WIN2000_BUTTON_SHADOW);
        assert_eq!(
            visuals.override_text_color,
            Some(Style::WIN2000_WINDOW_TEXT)
        );
        assert_eq!(
            visuals.widgets.hovered.bg_fill,
            Style::WIN2000_BUTTON_HIGHLIGHT
        );
        assert_eq!(
            visuals.widgets.active.bg_fill,
            Style::WIN2000_PRESSED_FACE,
            "light-mode pressed state uses classic gray, not dark-mode accent fill"
        );
        assert_eq!(
            visuals.widgets.active.fg_stroke.color,
            Style::WIN2000_WINDOW_TEXT,
            "strong text remains black and readable in light mode"
        );
        assert_eq!(
            visuals.hyperlink_color,
            Style::WIN2000_ACTIVE_TITLE,
            "default brand accent resolves to classic active-title blue"
        );
    }

    #[test]
    fn color_scheme_remaps_explicit_style_token_shapes() {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(egui::pos2(0.0, 0.0), Style::TEXT_DIM);
        mesh.colored_vertex(egui::pos2(1.0, 0.0), Style::SURFACE);
        mesh.colored_vertex(egui::pos2(0.0, 1.0), Style::ACCENT);

        let font_id = egui::FontId::new(Style::BODY, egui::FontFamily::Proportional);
        let mut job = egui::text::LayoutJob::default();
        job.append(
            "Token text",
            0.0,
            egui::TextFormat {
                font_id,
                color: Style::TEXT,
                background: Style::SURFACE,
                underline: egui::Stroke::new(1.0, Style::BORDER),
                ..Default::default()
            },
        );
        let ctx = egui::Context::default();
        let mut galley = None;
        let _ = ctx.run(Default::default(), |ctx| {
            Style::install(ctx);
            galley = Some(ctx.fonts(|fonts| fonts.layout_job(job.clone())));
        });
        let galley = galley.expect("headless frame laid out test text");
        let text = egui::epaint::TextShape::new(egui::pos2(0.0, 0.0), galley, Style::TEXT_DIM);

        let mut shapes = vec![
            egui::epaint::ClippedShape {
                clip_rect: egui::Rect::EVERYTHING,
                shape: egui::Shape::Rect(egui::epaint::RectShape::new(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(10.0)),
                    0.0,
                    Style::BG,
                    egui::Stroke::new(1.0, Style::BORDER),
                    egui::StrokeKind::Outside,
                )),
            },
            egui::epaint::ClippedShape {
                clip_rect: egui::Rect::EVERYTHING,
                shape: egui::Shape::Text(text),
            },
            egui::epaint::ClippedShape {
                clip_rect: egui::Rect::EVERYTHING,
                shape: egui::Shape::Mesh(mesh.into()),
            },
        ];

        Style::remap_clipped_shapes_for_color_scheme(&mut shapes, StyleColorScheme::Light);
        let p = Style::palette_for(StyleColorScheme::Light);
        match &shapes[0].shape {
            egui::Shape::Rect(rect) => {
                assert_eq!(rect.fill, p.bg);
                assert_eq!(rect.stroke.color, p.border);
            }
            other => panic!("unexpected rect shape: {other:?}"),
        }
        match &shapes[1].shape {
            egui::Shape::Text(text) => {
                assert_eq!(text.fallback_color, p.text_dim);
                assert_eq!(text.galley.job.sections[0].format.color, p.text);
                assert_eq!(text.galley.job.sections[0].format.background, p.surface);
                assert_eq!(text.galley.job.sections[0].format.underline.color, p.border);
            }
            other => panic!("unexpected text shape: {other:?}"),
        }
        match &shapes[2].shape {
            egui::Shape::Mesh(mesh) => {
                assert_eq!(mesh.vertices[0].color, p.text_dim);
                assert_eq!(mesh.vertices[1].color, p.surface);
                assert_eq!(mesh.vertices[2].color, Style::WIN2000_ACTIVE_TITLE);
            }
            other => panic!("unexpected mesh shape: {other:?}"),
        }
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
        assert_eq!(
            s.visuals.widgets.active.bg_fill,
            Style::pressed_fill(Style::ACCENT_MESH)
        );
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
        // The single mapping the shell keys off the SURFACE-9 signal. Compact and
        // Comfortable are user-selectable presets, so formfactor still resolves only
        // to the two anchor densities.
        assert_eq!(Density::for_formfactor(Formfactor::Tablet), Density::Touch);
        assert_eq!(Density::for_formfactor(Formfactor::Laptop), Density::Mouse);
    }

    #[test]
    fn four_density_presets_scale_spacing_monotonically() {
        // Compact < Mouse < Comfortable < Touch in spacing; hit targets step
        // pointer(24) → finger(44) once. Spacing is the only thing density moves —
        // component dimensions never key off it (lock #7 / UX-24).
        let ladder = [
            Density::Compact,
            Density::Mouse,
            Density::Comfortable,
            Density::Touch,
        ];
        for w in ladder.windows(2) {
            assert!(
                w[1].spacing_scale() > w[0].spacing_scale(),
                "{:?} spacing must exceed {:?}",
                w[1],
                w[0]
            );
        }
        assert_eq!(
            Density::Compact.min_hit_target(),
            Density::Mouse.min_hit_target()
        );
        assert_eq!(
            Density::Comfortable.min_hit_target(),
            Density::Touch.min_hit_target()
        );
        assert!(Density::Comfortable.min_hit_target() > Density::Mouse.min_hit_target());
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
    fn carbon_support_tokens_map_the_notification_taxonomy() {
        // NOTIF-1: Red = alert/action-needed, Amber = warning, Blue = info, and
        // success stays the existing OK green. These are semantic aliases, not a
        // second palette.
        assert_eq!(Style::SUPPORT_ERROR, Style::DANGER);
        assert_eq!(Style::SUPPORT_WARNING, Style::WARN);
        assert_eq!(Style::SUPPORT_SUCCESS, Style::OK);
        assert_eq!(Style::SUPPORT_INFO, Style::ACCENT);
        assert_ne!(Style::SUPPORT_ERROR, Style::SUPPORT_WARNING);
        assert_ne!(Style::SUPPORT_WARNING, Style::SUPPORT_INFO);
        assert_ne!(Style::SUPPORT_INFO, Style::SUPPORT_SUCCESS);
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
