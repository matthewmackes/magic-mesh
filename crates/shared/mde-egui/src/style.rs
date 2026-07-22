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
    Color32, Context, FontFamily, FontId, Stroke, TextStyle,
};
use serde::{Deserialize, Serialize};

use crate::formfactor::Formfactor;

/// The shell-wide layout profile. Profiles are not just density presets: each one
/// names a distinct placement model the shell can branch on while still sharing
/// the same `Style` palette and Inter-first font system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutProfile {
    /// Windows 2000 Workstation: classic bottom taskbar + Start lower-left.
    #[default]
    Workstation,
    /// Touch tablet: bottom touch bar, larger targets, slide-up controls.
    Tablet,
    /// Vehicle HUD: glanceable driving/vehicle/media/comms controls.
    Car,
}

impl LayoutProfile {
    /// Visible picker order.
    pub const ALL: [Self; 3] = [Self::Workstation, Self::Tablet, Self::Car];

    /// Human label for settings/menu rows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workstation => "Windows 2000 Workstation",
            Self::Tablet => "Tablet",
            Self::Car => "Car",
        }
    }

    /// Compact shell-control label for constrained mode buttons.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Workstation => "WS",
            Self::Tablet => "TAB",
            Self::Car => "CAR",
        }
    }

    /// Settings/menu description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Workstation => "Classic desktop placement",
            Self::Tablet => "Bottom touch bar and larger targets",
            Self::Car => "Driving HUD and keyboard actions",
        }
    }

    /// Runtime interaction density installed for this profile.
    #[must_use]
    pub const fn density(self) -> Density {
        match self {
            Self::Workstation => Density::Mouse,
            Self::Tablet | Self::Car => Density::Touch,
        }
    }

    /// Whether this profile is the vehicle HUD model.
    #[must_use]
    pub const fn is_car(self) -> bool {
        matches!(self, Self::Car)
    }
}

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
    /// **Ford Sync 3 Auto skin** — a black / white / blue in-vehicle palette,
    /// applied automatically while the [`LayoutProfile::Car`] Auto Mode is active
    /// (never a manually-pickable Personalization theme, so it is deliberately
    /// **absent from [`ALL`](Self::ALL)**). Deep black ground for glare-free night
    /// driving, pure-white glanceable text, and a bright Sync-3 blue accent.
    AutoSync3,
}

impl StyleColorScheme {
    /// Visible mode order in the Personalization → Theme picker. Deliberately only
    /// the two operator-pickable schemes — [`AutoSync3`](Self::AutoSync3) is
    /// mode-derived (installed by Car/Auto Mode), not a manual theme.
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

    // ── Palette (Ford Sync 3 Auto skin — black / white / blue) ───────────────
    // The in-vehicle [`StyleColorScheme::AutoSync3`] palette, applied while Car
    // Mode is active. A near-black ground (glare-free at night), pure-white
    // glanceable text, and a bright Sync-3 blue accent — the Ford SYNC 3 dark
    // interface's black/white/blue language. Kept as its own token block (like
    // `WIN2000_*`) so the scheme projection in `palette_for` reads named tokens,
    // never inline literals.
    /// Sync-3 deepest ground — near-black with a faint cool tint.
    pub const SYNC3_BG: Color32 = Color32::from_rgb(0x04, 0x07, 0x0B);
    /// Sync-3 raised tile / card charcoal.
    pub const SYNC3_SURFACE: Color32 = Color32::from_rgb(0x12, 0x17, 0x1E);
    /// Sync-3 hovered / highlighted tile.
    pub const SYNC3_SURFACE_HI: Color32 = Color32::from_rgb(0x1C, 0x24, 0x2E);
    /// Sync-3 cool hairline / separator.
    pub const SYNC3_BORDER: Color32 = Color32::from_rgb(0x2B, 0x35, 0x40);
    /// Sync-3 primary text — pure white for maximum at-a-glance contrast.
    pub const SYNC3_TEXT: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    /// Sync-3 secondary text — cool light gray.
    pub const SYNC3_TEXT_DIM: Color32 = Color32::from_rgb(0xA6, 0xB4, 0xC2);
    /// Sync-3 emphasis text — pure white.
    pub const SYNC3_TEXT_STRONG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    /// Sync-3 signature accent — a bright sky/cyan Ford SYNC blue.
    pub const SYNC3_ACCENT: Color32 = Color32::from_rgb(0x2E, 0x9B, 0xE6);
    /// Sync-3 accent highlight — one rung brighter for pressed rings.
    pub const SYNC3_ACCENT_HI: Color32 = Color32::from_rgb(0x5F, 0xB8, 0xF2);

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

    // ── Scrim materials (PLATFORM-INTERFACES Q21) ───────────────────────────
    // Three depths of layered translucency approximating the HIG material ladder
    // (thin / regular / thick) **without live blur** — the GLES/DRM path has no
    // blur budget, so a material here is honest alpha over the content beneath.
    // Each is [`BG`](Self::BG)'s near-black hue at a rising alpha (premultiplied
    // by hand so the consts stay `const`): three distinct push-back depths over
    // the Quazar-dark ground, hue-matched to it rather than pure black.
    /// **Thin material** — the lightest push-back (~55% [`BG`](Self::BG)): a
    /// hover veil / de-emphasis wash where the content beneath stays readable.
    /// `#16161A` at `0x8C`, premultiplied. Approximates HIG *thin material*
    /// without blur (Q21).
    pub const SCRIM_THIN: Color32 = Color32::from_rgba_premultiplied(0x0C, 0x0C, 0x0E, 0x8C);
    /// **Regular material** — the standard overlay scrim (~72% [`BG`](Self::BG)):
    /// sheets, popover backdrops — the layer beneath clearly recedes yet remains
    /// legible in silhouette. `#16161A` at `0xB8`, premultiplied. Approximates
    /// HIG *regular material* without blur (Q21).
    pub const SCRIM_REGULAR: Color32 = Color32::from_rgba_premultiplied(0x10, 0x10, 0x13, 0xB8);
    /// **Thick material** — the deepest push-back (~88% [`BG`](Self::BG)): modal
    /// focus / lock-adjacent overlays where the layer beneath is context only.
    /// `#16161A` at `0xE0`, premultiplied. Approximates HIG *thick material*
    /// without blur (Q21).
    pub const SCRIM_THICK: Color32 = Color32::from_rgba_premultiplied(0x13, 0x13, 0x17, 0xE0);

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

    // ── Bottom taskbar chrome ──────────────────────────────────────────────
    // The platform taskbar is deliberately a black shell strip with white glyphs
    // and a Windows-11-style tray island. Keep the exact palette in shared Style
    // so dock rendering and visual tests do not mint private shell colours.
    /// Full-width bottom taskbar strip.
    pub const TASKBAR_BG: Color32 = Color32::BLACK;
    /// Taskbar hairline/separator.
    pub const TASKBAR_BORDER: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
    /// Hover fill for taskbar-owned cells.
    pub const TASKBAR_HOVER_FILL: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
    /// Active/selected fill for taskbar-owned cells.
    pub const TASKBAR_ACTIVE_FILL: Color32 = Color32::from_rgb(0x2D, 0x2D, 0x2D);
    /// Taskbar-owned control glyph tint.
    pub const TASKBAR_ICON: Color32 = Color32::WHITE;
    /// Windows 11-style tray island fill.
    pub const TASKBAR_TRAY_ISLAND_FILL: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
    /// Active Windows 11-style tray island fill.
    pub const TASKBAR_TRAY_ISLAND_ACTIVE_FILL: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
    /// Windows 11-style tray island border.
    pub const TASKBAR_TRAY_ISLAND_BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
    /// Secondary date text in the taskbar clock stack.
    pub const TASKBAR_CLOCK_DATE: Color32 = Color32::from_rgb(0xD6, 0xD6, 0xD6);

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

    // ── Semantic interaction-state roles (UI-VIS-110) ───────────────────────
    // success/warning/error/info already have Carbon SUPPORT_* aliases above; a
    // control still cycles through focus / selected / disabled, so name those
    // remaining state roles once here — a surface must never re-decide "what
    // colour is focus / selection / disabled" at a call site, and state is never
    // communicated by colour alone (the focus *ring width* and the disabled
    // *dimming* carry the meaning too).
    /// **Focus** — the accent hue of the visible focus ring. The ring reads as
    /// focus (not hover/selection) by its 2 px [`FOCUS_RING_W`](Self::FOCUS_RING_W)
    /// width, per UI-VIS-132; build it with [`focus_stroke`](Self::focus_stroke).
    pub const FOCUS: Color32 = Self::ACCENT;
    /// **Selection** — the accent a selected row / tab / text run is keyed to; the
    /// translucent body fill comes from [`selection_fill`](Self::selection_fill).
    pub const SELECTION: Color32 = Self::ACCENT;
    /// **Disabled** — the quiet foreground for an unavailable/disabled control:
    /// one rung below [`TEXT_DIM`](Self::TEXT_DIM) so a disabled label reads as
    /// "unavailable", never as ordinary secondary text.
    pub const DISABLED: Color32 = Color32::from_rgb(0x5C, 0x5C, 0x66);

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
    // PLATFORM-INTERFACES Q23: the HIG radii ladder (~6/10/16/26) — continuous
    // curvature-era rounding replacing the Win10-era 4/6/8 squareness. Nested
    // rounded rects follow the concentric rule via
    // [`concentric_radius`](Self::concentric_radius).
    /// Tight radius — buttons, chips, taskbar/cell inner fills.
    pub const RADIUS_S: f32 = 6.0;
    /// Mid radius — cards, menus, popovers (the historical default).
    pub const RADIUS_M: f32 = 10.0;
    /// Large radius — windows, sheets, dialogs, the lock curtain.
    pub const RADIUS_L: f32 = 16.0;
    /// Extra-large radius — hero cards, modal sheets, springboard tiles (Q22/Q23).
    pub const RADIUS_XL: f32 = 26.0;
    /// Back-compat alias for the mid tier — the ~130 pre-tier call sites read this.
    pub const RADIUS: f32 = Self::RADIUS_M;
    /// The floor an inner concentric radius never drops below — a nested corner
    /// stays visibly rounded, it never collapses square (Q23).
    const RADIUS_CONCENTRIC_FLOOR: f32 = 2.0;

    /// PLATFORM-INTERFACES Q23 — the **concentric-nesting rule**: a rounded rect
    /// inset `inset` px inside a rounded parent shares the parent's *center of
    /// curvature*, so the inner radius is `outer_radius - inset` (floored at
    /// [`RADIUS_CONCENTRIC_FLOOR`](Self::RADIUS_CONCENTRIC_FLOOR) so a deep inset
    /// never yields a square or negative corner). One derivation, so no surface
    /// eyeballs an inner radius for a plate-in-card / glyph-in-tile nest (§4).
    #[must_use]
    pub const fn concentric_radius(outer_radius: f32, inset: f32) -> f32 {
        let inner = outer_radius - inset;
        if inner > Self::RADIUS_CONCENTRIC_FLOOR {
            inner
        } else {
            Self::RADIUS_CONCENTRIC_FLOOR
        }
    }

    // ── Stroke widths ───────────────────────────────────────────────────────
    /// **Hairline** — the 1 px weight of a border / separator ([`BORDER`](Self::BORDER)).
    /// The single source for `Stroke::new(1.0, …)` chrome; [`hairline`](Self::hairline)
    /// bakes it against the border tone.
    pub const STROKE_HAIRLINE: f32 = 1.0;
    /// **Focus-ring width** — the 2 px accent ring (the platform's 2 px focus-ring
    /// design lock); [`focus_stroke`](Self::focus_stroke) bakes it against
    /// [`FOCUS`](Self::FOCUS).
    pub const FOCUS_RING_W: f32 = 2.0;

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
    /// Top-left workspace title text: the display rung reduced by two points for
    /// refined shell chrome.
    pub const WORKSPACE_TITLE: f32 = Self::DISPLAY - 2.0;
    /// Shared menu/button chrome text: the body rung reduced by one point so
    /// menus read as controls, not body copy.
    pub const MENU_TEXT: f32 = Self::BODY - 1.0;

    // ── HIG semantic type ramp (PLATFORM-INTERFACES Q4) ─────────────────────
    // The HIG roles (Large Title → Caption) carried by the EXISTING Inter face
    // (the SF stand-in; Plex Mono stays the code/terminal face — no new font).
    // Where a role lands on an existing Carbon rung it ALIASES it (one scale,
    // not two): Title1 = DISPLAY, Headline = TITLE, Body = BODY, Caption =
    // SMALL. The ramp descends strictly, scaled to the platform's dense 12pt
    // body rather than HIG's 17pt paper sizes.
    /// HIG **Large Title** — the hero rung above [`DISPLAY`](Self::DISPLAY):
    /// springboard page titles, first-run heroes.
    pub const TYPE_LARGE_TITLE: f32 = 30.0;
    /// HIG **Title 1** — aliases the existing display rung ([`DISPLAY`](Self::DISPLAY)).
    pub const TYPE_TITLE1: f32 = Self::DISPLAY;
    /// HIG **Title 2** — between the display and section-heading rungs.
    pub const TYPE_TITLE2: f32 = 21.0;
    /// HIG **Title 3** — just under [`HEADING`](Self::HEADING).
    pub const TYPE_TITLE3: f32 = 19.0;
    /// HIG **Headline** — aliases the sub-heading rung ([`TITLE`](Self::TITLE)).
    /// HIG headline is *semibold*; the embedded Inter face has no bold cut, so
    /// the platform's honest emphasis cue is pairing this size with
    /// [`TEXT_STRONG`](Self::TEXT_STRONG) (the established weight substitute).
    pub const TYPE_HEADLINE: f32 = Self::TITLE;
    /// HIG **Body** — aliases the platform body rung ([`BODY`](Self::BODY)).
    pub const TYPE_BODY: f32 = Self::BODY;
    /// HIG **Callout** — emphasized standalone copy above body scale.
    pub const TYPE_CALLOUT: f32 = 15.0;
    /// HIG **Subheadline** — secondary line under a headline.
    pub const TYPE_SUBHEADLINE: f32 = 14.0;
    /// HIG **Footnote** — one point under body (the platform body is already
    /// footnote-dense, so the rung sits at 11 to keep the ramp strictly
    /// descending rather than tying [`TYPE_BODY`](Self::TYPE_BODY)).
    pub const TYPE_FOOTNOTE: f32 = 11.0;
    /// HIG **Caption** — aliases the existing small/caption rung ([`SMALL`](Self::SMALL)).
    pub const TYPE_CAPTION: f32 = Self::SMALL;

    // ── Icon sizes (logical points) ─────────────────────────────────────────
    // The optical-size ladder for glyphs (Carbon icons render crisp at any of
    // these; see [`crate::carbon`]). One scale so a toolbar glyph, a menu glyph,
    // and a status dot never each pick a private pixel size (UI-VIS-119/120).
    /// Small icon — inline status affordances, dense rows.
    pub const ICON_S: f32 = 14.0;
    /// Medium icon — the default toolbar / menu glyph optical size.
    pub const ICON_M: f32 = 16.0;
    /// Large icon — prominent chrome actions, section headers.
    pub const ICON_L: f32 = 20.0;
    /// Extra-large icon — launcher tiles, hero / empty-state glyphs, touch targets.
    pub const ICON_XL: f32 = 24.0;
    /// Vertical padding for ordinary egui buttons. Kept well below the base 8px
    /// gutter so toolbar rows read refined without changing the minimum hit target.
    pub const CONTROL_PAD_Y: f32 = Self::SP_XS;
    /// Decorative vertical inset for stacked toolbar/header strips. This is
    /// intentionally near-zero; the hit target still comes from egui's
    /// interaction size, not from toolbar padding.
    pub const TOOLBAR_INSET_Y: f32 = 0.0;
    /// Refined pointer-toolbar visual control height. Derived from the menu text
    /// rung plus a compact vertical cushion so local toolbar rows can slim down
    /// without carrying unrelated 24pt literals.
    pub const TOOLBAR_CONTROL_H: f32 = Self::MENU_TEXT + Self::SP_S + Self::SP_XS * 0.5;

    // ── Control-height scale (UI-VIS-107) ───────────────────────────────────
    // Ordinary controls key a consistent, usable height off this small ladder
    // instead of stray per-widget literals. Density still owns the interaction
    // *hit-target floor* ([`Density::min_hit_target`]) and *spacing*; these are the
    // drawn control heights a surface sizes buttons / inputs / rows to.
    /// Compact control height — the dense/expert rung; equals the pointer
    /// hit-target floor ([`Density::Mouse`] `min_hit_target`).
    pub const CONTROL_H_S: f32 = 24.0;
    /// Standard control height — the roomier default for ordinary controls.
    pub const CONTROL_H_M: f32 = 28.0;
    /// Large control height — primary buttons and full list rows.
    pub const CONTROL_H_L: f32 = 36.0;

    /// A shared toolbar/header frame margin: normal horizontal inset, refined
    /// vertical inset. Use this for chrome strips that surround controls, not for
    /// content cards or body panels.
    #[must_use]
    pub const fn toolbar_margin() -> egui::Margin {
        egui::Margin::symmetric(Self::SP_XS as i8, Self::TOOLBAR_INSET_Y as i8)
    }

    /// Shared tooltip/hover-card frame margin. This keeps transient chrome lighter
    /// than content cards while still leaving enough breathing room for small text.
    #[must_use]
    pub const fn tooltip_margin() -> egui::Margin {
        egui::Margin::symmetric(Self::SP_S as i8, Self::SP_XS as i8)
    }

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
            s.text_styles.insert(
                TextStyle::Small,
                FontId::new(Self::SMALL, FontFamily::Proportional),
            );
            s.text_styles.insert(
                TextStyle::Body,
                FontId::new(Self::BODY, FontFamily::Proportional),
            );
            s.text_styles.insert(
                TextStyle::Button,
                FontId::new(Self::MENU_TEXT, FontFamily::Proportional),
            );
            s.text_styles.insert(
                TextStyle::Heading,
                FontId::new(Self::HEADING, FontFamily::Proportional),
            );
            s.text_styles.insert(
                TextStyle::Monospace,
                FontId::new(Self::BODY, FontFamily::Monospace),
            );
            s.spacing.item_spacing = egui::vec2(Self::SP_S * sp, Self::SP_S * sp);
            s.spacing.button_padding = egui::vec2(Self::SP_M * sp, Self::CONTROL_PAD_Y * sp);
            s.spacing.indent = Self::SP_M * sp;
            // The minimum interactive size is the finger/pointer hit target.
            s.spacing.interact_size.y = density.min_hit_target();
        });
        ctx.data_mut(|d| {
            d.insert_temp(Self::color_scheme_id(), scheme);
            d.insert_temp(Self::density_id(), density);
        });
    }

    /// The current colour mode installed on `ctx`.
    #[must_use]
    pub fn color_scheme(ctx: &Context) -> StyleColorScheme {
        ctx.data(|d| {
            d.get_temp::<StyleColorScheme>(Self::color_scheme_id())
                .unwrap_or_default()
        })
    }

    /// The current density installed on `ctx`.
    #[must_use]
    pub fn density(ctx: &Context) -> Density {
        ctx.data(|d| {
            d.get_temp::<Density>(Self::density_id())
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
            StyleColorScheme::AutoSync3 => StylePalette {
                bg: Self::SYNC3_BG,
                surface: Self::SYNC3_SURFACE,
                surface_hi: Self::SYNC3_SURFACE_HI,
                border: Self::SYNC3_BORDER,
                capture_clear: Self::SYNC3_BG,
                text: Self::SYNC3_TEXT,
                text_dim: Self::SYNC3_TEXT_DIM,
                text_strong: Self::SYNC3_TEXT_STRONG,
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
            Self::ACCENT => match scheme {
                StyleColorScheme::AutoSync3 => Self::SYNC3_ACCENT,
                _ => Self::WIN2000_ACTIVE_TITLE,
            },
            Self::ACCENT_HI => match scheme {
                StyleColorScheme::AutoSync3 => Self::SYNC3_ACCENT_HI,
                _ => Self::WIN2000_ACTIVE_TITLE,
            },
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

    fn density_id() -> egui::Id {
        egui::Id::new("mde-egui-style-density")
    }

    fn visuals_for(scheme: StyleColorScheme, accent: Color32, accent_hi: Color32) -> egui::Visuals {
        let p = Self::palette_for(scheme);
        let mut v = match scheme {
            // Sync-3 is a dark-on-black skin, so it derives from the dark base.
            StyleColorScheme::Dark | StyleColorScheme::AutoSync3 => egui::Visuals::dark(),
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

        // Corner geometry + elevation from the shared tokens (UI-VIS-105/108) so
        // no surface hand-rolls widget rounding or a popup/window shadow.
        Self::apply_geometry(&mut v);
        v
    }

    /// Configure the shared corner geometry + elevation on `v` from the radius and
    /// shadow tokens (UI-VIS-105/106/108): controls take the small radius, menus
    /// the mid radius, windows/dialogs the large radius; anchored popups and
    /// windows cast the shared soft overlay / modal shadow. The single place the
    /// installed [`egui::Visuals`] gets its rounding + depth, so a surface never
    /// re-decides widget rounding at a call site (it was hand-set as
    /// `visuals.menu_corner_radius = …` in at least one surface before this).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn apply_geometry(v: &mut egui::Visuals) {
        let control = egui::CornerRadius::same(Self::RADIUS_S as u8);
        v.widgets.noninteractive.corner_radius = control;
        v.widgets.inactive.corner_radius = control;
        v.widgets.hovered.corner_radius = control;
        v.widgets.active.corner_radius = control;
        v.widgets.open.corner_radius = control;
        v.window_corner_radius = egui::CornerRadius::same(Self::RADIUS_L as u8);
        v.menu_corner_radius = egui::CornerRadius::same(Self::RADIUS_M as u8);
        v.popup_shadow = Elevation::Overlay.egui_shadow();
        v.window_shadow = Elevation::Modal.egui_shadow();
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
        match scheme {
            // Dark keeps every accent exactly as chosen.
            StyleColorScheme::Dark => accent,
            // Windows-2000 light folds the DEFAULT brand accent to the classic
            // active-title blue; an explicit user accent pick keeps its hue.
            StyleColorScheme::Light if accent == Self::ACCENT => Self::WIN2000_ACTIVE_TITLE,
            StyleColorScheme::Light => accent,
            // Sync-3 folds the default brand accent (both rungs) to the bright
            // Ford SYNC blue; a user pick keeps its hue.
            StyleColorScheme::AutoSync3 if accent == Self::ACCENT => Self::SYNC3_ACCENT,
            StyleColorScheme::AutoSync3 if accent == Self::ACCENT_HI => Self::SYNC3_ACCENT_HI,
            StyleColorScheme::AutoSync3 => accent,
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
            // Sync-3's near-black ground darkens the accent toward black just like
            // dark mode, keeping the bright pressed label WCAG-legible.
            StyleColorScheme::Dark | StyleColorScheme::AutoSync3 => Self::pressed_fill(accent),
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

    /// The **row / text selection** fill — the accent at a body alpha, matching
    /// the `selection.bg_fill` the shared install lands on egui. Heavier than the
    /// light [`selection_wash`](Self::selection_wash) marquee tint: use this for a
    /// selected list row / tab / text run, the wash for a drag-select rectangle.
    /// One source, so a selected surface never re-mixes the accent alpha (§4).
    #[must_use]
    pub fn selection_fill() -> Color32 {
        Self::ACCENT.gamma_multiply(0.35)
    }

    // ── Springboard tile plates (PLATFORM-INTERFACES Q22) ───────────────────
    // The home-grid tile treatment: a rounded-rect plate in the group's accent,
    // a white Carbon glyph on it, the label beneath. The plate is the accent
    // composited over [`BG`](Self::BG) at [`TILE_PLATE_ALPHA`](Self::TILE_PLATE_ALPHA)
    // and flattened opaque at derivation time — darkened/desaturated enough that
    // the white glyph clears WCAG AA (≥ 4.5:1) on EVERY group accent, with no
    // runtime translucency cost (verified by `tile_glyph_stays_wcag_legible_on_
    // every_group_plate`).
    /// Q22 — the accent's compositing weight in a tile plate: the fraction of
    /// the group accent kept when it is laid over [`BG`](Self::BG) and flattened.
    /// Chosen so the brightest accent (gold) still holds the white glyph ≥ 4.5:1.
    pub const TILE_PLATE_ALPHA: f32 = 0.38;
    /// Q22 — the springboard tile **glyph** tint: white Carbon linework on the
    /// accent plate, one silhouette language across every group.
    pub const TILE_GLYPH: Color32 = Color32::WHITE;

    /// Q22 — a springboard tile's **plate fill**: `accent` composited over
    /// [`BG`](Self::BG) at [`TILE_PLATE_ALPHA`](Self::TILE_PLATE_ALPHA), opaque.
    /// The single derivation, so a tile never re-mixes its plate at a call site
    /// (§4) and a future accent automatically inherits the contrast guarantee.
    #[must_use]
    pub fn tile_plate_fill(accent: Color32) -> Color32 {
        Self::blend(accent, Self::BG, 1.0 - Self::TILE_PLATE_ALPHA)
    }

    /// The **2 px accent focus ring** stroke ([`FOCUS`](Self::FOCUS) at
    /// [`FOCUS_RING_W`](Self::FOCUS_RING_W)) — the platform focus-ring design lock.
    /// One source so every surface draws the same visible-focus treatment,
    /// distinct from hover/selection by its width (UI-VIS-132).
    #[must_use]
    pub fn focus_stroke() -> Stroke {
        Stroke::new(Self::FOCUS_RING_W, Self::FOCUS)
    }

    /// The **hairline border** stroke — [`BORDER`](Self::BORDER) at
    /// [`STROKE_HAIRLINE`](Self::STROKE_HAIRLINE) — so a surface never re-types
    /// `Stroke::new(1.0, Style::BORDER)` for a separator / card edge.
    #[must_use]
    pub fn hairline() -> Stroke {
        Stroke::new(Self::STROKE_HAIRLINE, Self::BORDER)
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

/// A soft-shadow token (raw data). A surface (or the shared
/// [`to_shadow`](ShadowToken::to_shadow) converter) turns it into an
/// `egui::epaint::Shadow` at draw time. The umbra is
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

    /// This tier's soft shadow as a concrete [`egui::epaint::Shadow`], ready for
    /// [`egui::Frame::shadow`] — the ergonomic one-call form of
    /// `self.shadow().to_shadow()`. [`Flat`](Self::Flat) yields a fully-transparent
    /// (no-op) shadow. This is the single shared `Elevation → Shadow` conversion,
    /// superseding the per-surface `card_shadow()` helper that was copy-pasted
    /// across several shell surfaces.
    #[must_use]
    pub fn egui_shadow(self) -> egui::epaint::Shadow {
        self.shadow().to_shadow()
    }
}

impl ShadowToken {
    /// Build the concrete [`egui::epaint::Shadow`] this token describes, mapping
    /// the token's logical-px `f32` fields onto egui's `i8`/`u8` shadow fields.
    /// The one shared converter, so a surface reads a soft-shadow token straight
    /// into a `Frame` instead of re-typing the field mapping by hand.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn to_shadow(self) -> egui::epaint::Shadow {
        egui::epaint::Shadow {
            offset: [self.offset[0] as i8, self.offset[1] as i8],
            blur: self.blur as u8,
            spread: self.spread as u8,
            color: self.umbra,
        }
    }
}

/// The restrained **three-level surface hierarchy** (UI-VIS-106): the app
/// background, a persistent base surface, and an elevated/floating surface — the
/// only three tonal planes a shell should stack, so regions separate by
/// elevation instead of boxing every panel. Each level resolves onto an existing
/// palette tone (§4 — no new hue) and the [`Elevation`] that casts its shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceLevel {
    /// The deepest plane — the desktop / window background ([`Style::BG`]).
    App,
    /// A persistent panel / page surface resting on the app background
    /// ([`Style::SURFACE`], Carbon layer-01).
    Base,
    /// A raised card / floating surface one step above the base
    /// ([`Style::SURFACE_HI`], Carbon layer-02).
    Elevated,
}

impl SurfaceLevel {
    /// Ordered deepest → highest.
    pub const ALL: [Self; 3] = [Self::App, Self::Base, Self::Elevated];

    /// The palette fill for this plane.
    #[must_use]
    pub const fn fill(self) -> Color32 {
        match self {
            Self::App => Style::BG,
            Self::Base => Style::SURFACE,
            Self::Elevated => Style::SURFACE_HI,
        }
    }

    /// The elevation (shadow depth) this plane casts. The two grounded planes are
    /// [`Flat`](Elevation::Flat); only the elevated surface lifts off the page.
    #[must_use]
    pub const fn elevation(self) -> Elevation {
        match self {
            Self::App | Self::Base => Elevation::Flat,
            Self::Elevated => Elevation::Raised,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::assertions_on_constants,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::{
        Density, Elevation, GradeBand, LayoutProfile, ShadowToken, Style, StyleColorScheme,
        SurfaceLevel,
    };
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
        // PLATFORM-INTERFACES Q23: the HIG ladder (~6/10/16/26) — strictly
        // ascending, each on the 2px sub-grid, mid == the back-compat alias.
        assert!(
            Style::RADIUS_S < Style::RADIUS_M
                && Style::RADIUS_M < Style::RADIUS_L
                && Style::RADIUS_L < Style::RADIUS_XL,
            "radius tiers must strictly ascend: {} < {} < {} < {}",
            Style::RADIUS_S,
            Style::RADIUS_M,
            Style::RADIUS_L,
            Style::RADIUS_XL,
        );
        for r in [
            Style::RADIUS_S,
            Style::RADIUS_M,
            Style::RADIUS_L,
            Style::RADIUS_XL,
        ] {
            assert_eq!(r % 2.0, 0.0, "{r} is off the 2px sub-grid");
        }
        assert_eq!(
            Style::RADIUS,
            Style::RADIUS_M,
            "RADIUS must alias the mid tier so pre-tier call sites are unchanged"
        );
    }

    #[test]
    fn concentric_radius_shares_the_center_of_curvature() {
        // Q23 — the concentric rule: inner = outer − inset, so nested rounded
        // rects share one center of curvature...
        assert_eq!(
            Style::concentric_radius(Style::RADIUS_XL, Style::SP_S),
            Style::RADIUS_XL - Style::SP_S
        );
        assert_eq!(
            Style::concentric_radius(Style::RADIUS_L, 4.0),
            Style::RADIUS_L - 4.0
        );
        // ...a zero inset is the identity...
        assert_eq!(
            Style::concentric_radius(Style::RADIUS_M, 0.0),
            Style::RADIUS_M
        );
        // ...and a deep inset floors at a still-rounded 2.0, never square/negative.
        assert_eq!(Style::concentric_radius(Style::RADIUS_S, 10.0), 2.0);
        assert_eq!(
            Style::concentric_radius(Style::RADIUS_S, Style::RADIUS_S),
            2.0
        );
    }

    #[test]
    fn scrim_materials_form_three_translucent_depths() {
        // Q21 — thin/regular/thick approximate the HIG material ladder as
        // BG-hued alpha (no live blur on GLES/DRM): every rung translucent,
        // depths strictly deepen, and each is a distinct token.
        let ladder = [Style::SCRIM_THIN, Style::SCRIM_REGULAR, Style::SCRIM_THICK];
        for s in ladder {
            assert!(
                s.a() > 0 && s.a() < 255,
                "a scrim material is translucent, not opaque: a={}",
                s.a()
            );
            // Valid premultiplied encoding: no channel exceeds the alpha.
            assert!(
                s.r() <= s.a() && s.g() <= s.a() && s.b() <= s.a(),
                "premultiplied channels must not exceed alpha"
            );
            // BG-hued (blue-leaning near-black like #16161A), not pure black.
            assert!(
                s.b() >= s.r() && s.r() == s.g(),
                "a scrim keeps BG's cool near-black hue"
            );
        }
        for w in ladder.windows(2) {
            assert!(
                w[1].a() > w[0].a(),
                "scrim depths must strictly deepen thin → regular → thick"
            );
            assert_ne!(w[0], w[1], "each material is its own distinct depth");
        }
    }

    #[test]
    fn tile_glyph_stays_wcag_legible_on_every_group_plate() {
        // Q22 — the white Carbon glyph must clear the WCAG AA body floor
        // (≥ 4.5:1) on the tile plate derived from EVERY group accent — the
        // brightest (gold) is the worst case, but guard them all so a future
        // accent inherits the guarantee.
        const AA_BODY: f32 = 4.5;
        assert_eq!(Style::TILE_GLYPH, egui::Color32::WHITE);
        assert!(
            Style::TILE_PLATE_ALPHA > 0.0 && Style::TILE_PLATE_ALPHA < 1.0,
            "the plate keeps a real fraction of the accent"
        );
        let accents = [
            ("ACCENT (Brand)", Style::ACCENT),
            ("ACCENT_COMMS (Cyan)", Style::ACCENT_COMMS),
            ("ACCENT_WORKLOADS (Purple)", Style::ACCENT_WORKLOADS),
            ("ACCENT_TERMINALS (Teal)", Style::ACCENT_TERMINALS),
            ("ACCENT_WEB (Chrome blue)", Style::ACCENT_WEB),
            ("ACCENT_MESH (Green)", Style::ACCENT_MESH),
            ("ACCENT_SYSTEM (Gold)", Style::ACCENT_SYSTEM),
            ("ACCENT_MEDIA (Magenta)", Style::ACCENT_MEDIA),
        ];
        for (name, accent) in accents {
            let plate = Style::tile_plate_fill(accent);
            assert_eq!(plate.a(), 0xFF, "a tile plate is flattened opaque");
            assert_ne!(plate, accent, "the plate is darkened, not the raw accent");
            let ratio = wcag_contrast_ratio(Style::TILE_GLYPH, plate);
            assert!(
                ratio >= AA_BODY,
                "white tile glyph over the {name} plate is only {ratio:.2}:1 — \
                 below the WCAG AA floor of {AA_BODY}:1"
            );
        }
    }

    #[test]
    fn hig_type_ramp_descends_onto_the_existing_scale() {
        // PLATFORM-INTERFACES Q4: the HIG roles descend strictly Large Title →
        // Caption, and every role that lands on an existing Carbon rung aliases
        // it — one type scale, not two.
        let ramp = [
            Style::TYPE_LARGE_TITLE,
            Style::TYPE_TITLE1,
            Style::TYPE_TITLE2,
            Style::TYPE_TITLE3,
            Style::TYPE_HEADLINE,
            Style::TYPE_CALLOUT,
            Style::TYPE_SUBHEADLINE,
            Style::TYPE_BODY,
            Style::TYPE_FOOTNOTE,
            Style::TYPE_CAPTION,
        ];
        for w in ramp.windows(2) {
            assert!(
                w[0] > w[1],
                "the HIG type ramp must strictly descend: {} !> {}",
                w[0],
                w[1]
            );
        }
        assert_eq!(Style::TYPE_TITLE1, Style::DISPLAY, "Title1 aliases DISPLAY");
        assert_eq!(Style::TYPE_HEADLINE, Style::TITLE, "Headline aliases TITLE");
        assert_eq!(Style::TYPE_BODY, Style::BODY, "Body aliases BODY");
        assert_eq!(Style::TYPE_CAPTION, Style::SMALL, "Caption aliases SMALL");
        assert!(
            Style::TYPE_LARGE_TITLE > Style::DISPLAY,
            "Large Title is the hero rung above the display size"
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
    fn taskbar_palette_keeps_black_bar_white_glyphs_and_grouped_tray() {
        assert_eq!(Style::TASKBAR_BG, egui::Color32::BLACK);
        assert_eq!(Style::TASKBAR_ICON, egui::Color32::WHITE);
        assert_ne!(Style::TASKBAR_BORDER, Style::TASKBAR_BG);
        assert_ne!(Style::TASKBAR_HOVER_FILL, Style::TASKBAR_BG);
        assert_ne!(Style::TASKBAR_ACTIVE_FILL, Style::TASKBAR_HOVER_FILL);
        assert_ne!(
            Style::TASKBAR_TRAY_ISLAND_ACTIVE_FILL,
            Style::TASKBAR_TRAY_ISLAND_FILL,
            "active tray island needs its own tone"
        );
        assert_ne!(
            Style::TASKBAR_TRAY_ISLAND_BORDER,
            Style::TASKBAR_TRAY_ISLAND_FILL,
            "tray island border must remain visible on the island fill"
        );
        assert!(
            wcag_contrast_ratio(Style::TASKBAR_ICON, Style::TASKBAR_BG) >= 7.0,
            "white taskbar icons must remain high contrast on the black bar"
        );
        assert!(
            wcag_contrast_ratio(Style::TASKBAR_CLOCK_DATE, Style::TASKBAR_BG) >= 7.0,
            "taskbar date text must remain readable on the black bar"
        );
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
        assert_eq!(
            Style::MENU_TEXT,
            Style::BODY - 1.0,
            "menu/control chrome text is one point below body text"
        );
        assert!(
            Style::SMALL < Style::MENU_TEXT && Style::MENU_TEXT < Style::BODY,
            "menu text should stay between captions and body copy"
        );
        assert_eq!(
            Style::WORKSPACE_TITLE,
            Style::DISPLAY - 2.0,
            "top-left workspace title chrome is two points below display text"
        );
        assert!(
            Style::HEADING < Style::WORKSPACE_TITLE && Style::WORKSPACE_TITLE < Style::DISPLAY,
            "workspace title should stay between section and display headings"
        );
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
        assert_eq!(
            ctx.style().text_styles[&egui::TextStyle::Body].size,
            Style::BODY
        );
        assert_eq!(
            ctx.style().text_styles[&egui::TextStyle::Button].size,
            Style::MENU_TEXT,
            "raw egui buttons and stray menu rows inherit the refined chrome text size"
        );
        assert_eq!(
            ctx.style().text_styles[&egui::TextStyle::Heading].size,
            Style::HEADING
        );
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
    fn auto_sync3_install_uses_black_white_blue_palette() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, StyleColorScheme::AutoSync3, Density::Touch);
        let visuals = &ctx.style().visuals;
        let p = Style::palette_for(StyleColorScheme::AutoSync3);

        // The Ford SYNC 3 Auto skin: near-black ground, pure-white text, blue accent.
        assert_eq!(Style::color_scheme(&ctx), StyleColorScheme::AutoSync3);
        assert_eq!(p.bg, Style::SYNC3_BG);
        assert_eq!(p.text, Style::SYNC3_TEXT);
        assert_eq!(
            p.text,
            egui::Color32::WHITE,
            "Sync-3 primary text is pure white"
        );
        assert_eq!(visuals.panel_fill, Style::SYNC3_BG);
        assert_eq!(visuals.extreme_bg_color, Style::SYNC3_BG);
        assert_eq!(visuals.override_text_color, Some(Style::SYNC3_TEXT));
        assert_eq!(
            visuals.hyperlink_color,
            Style::SYNC3_ACCENT,
            "the default brand accent resolves to the bright Ford SYNC blue"
        );
        assert_eq!(
            visuals.widgets.active.bg_stroke.color,
            Style::SYNC3_ACCENT_HI,
            "the pressed ring is the Sync-3 accent highlight"
        );

        // The near-black ground is strictly darker than the Construct-dark ground,
        // so the Sync-3 skin reads as its own blacker night palette, not dark mode.
        let luma = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(
            luma(Style::SYNC3_BG) < luma(Style::BG),
            "Sync-3 ground is blacker than Construct dark"
        );

        // The DRM shape-remap path resolves the static brand accent to the blue too.
        assert_eq!(
            Style::resolve_color_for_scheme(StyleColorScheme::AutoSync3, Style::ACCENT),
            Style::SYNC3_ACCENT
        );
        assert_eq!(
            Style::resolve_color_for_scheme(StyleColorScheme::AutoSync3, Style::TEXT),
            Style::SYNC3_TEXT
        );

        // AutoSync3 is mode-derived, never an operator-pickable Personalization theme.
        assert!(
            !StyleColorScheme::ALL.contains(&StyleColorScheme::AutoSync3),
            "Sync-3 is installed by Car Mode, not offered in the theme picker"
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
    fn layout_profiles_have_locked_order_and_density() {
        assert_eq!(
            LayoutProfile::ALL,
            [
                LayoutProfile::Workstation,
                LayoutProfile::Tablet,
                LayoutProfile::Car
            ]
        );
        assert_eq!(LayoutProfile::default(), LayoutProfile::Workstation);
        assert_eq!(LayoutProfile::Workstation.density(), Density::Mouse);
        assert_eq!(LayoutProfile::Tablet.density(), Density::Touch);
        assert_eq!(LayoutProfile::Car.density(), Density::Touch);
        assert!(LayoutProfile::Car.is_car());
        assert_eq!(LayoutProfile::Workstation.short_label(), "WS");
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

    #[test]
    fn button_padding_keeps_toolbars_refined_without_shrinking_hit_targets() {
        assert!(
            Style::CONTROL_PAD_Y < Style::SP_S,
            "toolbar buttons should use a slimmer vertical pad than the base gutter"
        );
        assert_eq!(
            Style::CONTROL_PAD_Y,
            Style::SP_XS,
            "refined toolbar controls keep to the half-gutter vertical padding"
        );
        assert!(
            Style::TOOLBAR_INSET_Y < Style::SP_XS,
            "stacked toolbar strips should use a refined vertical inset"
        );
        assert_eq!(
            Style::TOOLBAR_INSET_Y,
            0.0,
            "toolbar strip chrome should not add decorative vertical bulk"
        );
        assert_eq!(
            Style::TOOLBAR_CONTROL_H,
            Style::MENU_TEXT + Style::SP_S + Style::SP_XS * 0.5,
            "refined toolbar visual controls derive from the menu text rung"
        );
        assert!(
            Style::TOOLBAR_CONTROL_H < Density::Mouse.min_hit_target(),
            "the refined visual height stays slimmer than the pointer hit-target floor"
        );
        assert_eq!(
            Style::toolbar_margin(),
            egui::Margin::symmetric(Style::SP_XS as i8, Style::TOOLBAR_INSET_Y as i8)
        );

        let ctx = egui::Context::default();
        Style::install_with_density(&ctx, Density::Mouse);
        assert_eq!(
            ctx.style().spacing.button_padding.y,
            Style::CONTROL_PAD_Y,
            "the shared egui install owns the uniform toolbar/button vertical padding"
        );
        assert_eq!(
            ctx.style().spacing.interact_size.y,
            Density::Mouse.min_hit_target(),
            "refined padding must not reduce the minimum pointer hit target"
        );
    }

    #[test]
    fn tooltip_margin_stays_compact_and_uniform() {
        assert_eq!(
            Style::tooltip_margin(),
            egui::Margin::symmetric(Style::SP_S as i8, Style::SP_XS as i8)
        );
        assert!(
            Style::tooltip_margin().top < Style::SP_S as i8,
            "tooltips should not inherit thick content-card vertical padding"
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

    // --- UI-VIS-104..110: the polish foundation additions --------------------

    #[test]
    fn semantic_state_roles_are_named_and_distinct() {
        // Focus + selection key off the interactive accent (one interaction hue),
        // but each is its own named role so a surface reads intent, not a colour.
        assert_eq!(Style::FOCUS, Style::ACCENT);
        assert_eq!(Style::SELECTION, Style::ACCENT);
        // Disabled is a genuinely distinct quiet tone — dimmer than secondary
        // text, and not any of the surface/border/text tokens.
        let sum = |c: egui::Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(
            sum(Style::DISABLED) < sum(Style::TEXT_DIM),
            "disabled fg must be quieter than secondary text"
        );
        for other in [
            Style::TEXT,
            Style::TEXT_DIM,
            Style::TEXT_STRONG,
            Style::BORDER,
            Style::SURFACE,
            Style::SURFACE_HI,
        ] {
            assert_ne!(Style::DISABLED, other, "disabled must be its own tone");
        }
    }

    #[test]
    fn stroke_widths_name_the_hairline_and_2px_focus_ring() {
        assert_eq!(Style::STROKE_HAIRLINE, 1.0);
        assert_eq!(Style::FOCUS_RING_W, 2.0, "the focus ring is the 2px lock");
        let focus = Style::focus_stroke();
        assert_eq!(focus.width, Style::FOCUS_RING_W);
        assert_eq!(focus.color, Style::FOCUS);
        let hair = Style::hairline();
        assert_eq!(hair.width, Style::STROKE_HAIRLINE);
        assert_eq!(hair.color, Style::BORDER);
    }

    #[test]
    fn selection_fill_is_a_heavier_translucent_accent_than_the_wash() {
        // The row/text selection fill matches the installed selection.bg_fill and
        // is heavier than the light drag-select marquee wash — both accent-derived
        // so a theme re-tint carries them.
        let fill = Style::selection_fill();
        let wash = Style::selection_wash();
        assert!(
            fill.a() > 0 && fill.a() < 255,
            "selection fill is translucent"
        );
        assert!(
            fill.a() > wash.a(),
            "the selection fill ({}) must be heavier than the marquee wash ({})",
            fill.a(),
            wash.a()
        );
        assert!(
            fill.b() > fill.r(),
            "the selection fill keeps the accent hue"
        );
    }

    #[test]
    fn icon_sizes_ascend_on_a_single_ladder() {
        let ladder = [Style::ICON_S, Style::ICON_M, Style::ICON_L, Style::ICON_XL];
        for w in ladder.windows(2) {
            assert!(w[1] > w[0], "icon sizes must strictly ascend: {ladder:?}");
        }
        assert!(Style::ICON_S > 0.0, "the smallest icon size is positive");
    }

    #[test]
    fn control_height_scale_ascends_and_anchors_the_pointer_target() {
        assert!(
            Style::CONTROL_H_S < Style::CONTROL_H_M && Style::CONTROL_H_M < Style::CONTROL_H_L,
            "control heights must strictly ascend"
        );
        assert_eq!(
            Style::CONTROL_H_S,
            Density::Mouse.min_hit_target(),
            "the compact control height equals the pointer hit-target floor"
        );
    }

    #[test]
    fn surface_hierarchy_has_three_ascending_planes() {
        // Deepest → highest, each a distinct existing palette tone, only the
        // elevated plane lifts off the page.
        assert_eq!(
            SurfaceLevel::ALL,
            [
                SurfaceLevel::App,
                SurfaceLevel::Base,
                SurfaceLevel::Elevated
            ]
        );
        assert_eq!(SurfaceLevel::App.fill(), Style::BG);
        assert_eq!(SurfaceLevel::Base.fill(), Style::SURFACE);
        assert_eq!(SurfaceLevel::Elevated.fill(), Style::SURFACE_HI);
        assert_ne!(SurfaceLevel::App.fill(), SurfaceLevel::Base.fill());
        assert_ne!(SurfaceLevel::Base.fill(), SurfaceLevel::Elevated.fill());
        assert_eq!(SurfaceLevel::App.elevation(), Elevation::Flat);
        assert_eq!(SurfaceLevel::Base.elevation(), Elevation::Flat);
        assert_eq!(SurfaceLevel::Elevated.elevation(), Elevation::Raised);
    }

    #[test]
    fn shadow_token_converts_to_a_soft_egui_shadow() {
        // The shared Elevation → egui::Shadow converter matches the raw token
        // fields, stays soft (translucent) for real tiers, and is a no-op for Flat.
        for tier in [Elevation::Raised, Elevation::Overlay, Elevation::Modal] {
            let token = tier.shadow();
            let shadow = tier.egui_shadow();
            assert_eq!(shadow.blur, token.blur as u8);
            assert_eq!(shadow.color, token.umbra);
            assert!(shadow.blur > 0, "a real tier casts a blurred shadow");
            assert!(
                shadow.color.a() > 0 && shadow.color.a() < 255,
                "the umbra stays translucent"
            );
        }
        let flat = Elevation::Flat.egui_shadow();
        assert_eq!(flat.blur, 0, "flat casts no blur");
        assert_eq!(flat.color.a(), 0, "flat casts a transparent (no-op) shadow");
        // Depth grows monotonically Raised < Overlay < Modal.
        assert!(Elevation::Raised.egui_shadow().blur < Elevation::Overlay.egui_shadow().blur);
        assert!(Elevation::Overlay.egui_shadow().blur < Elevation::Modal.egui_shadow().blur);
        // The free token also converts (raw-data entry point).
        let raw: ShadowToken = Elevation::Modal.shadow();
        assert_eq!(raw.to_shadow(), Elevation::Modal.egui_shadow());
    }

    #[test]
    fn install_lands_the_shared_corner_geometry_and_shadows() {
        // UI-VIS-105/108: the authoritative install configures widget rounding,
        // menu/window rounding, and the popup/window shadow from the tokens — so a
        // surface inherits coherent geometry without hand-setting it.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let v = &ctx.style().visuals;
        assert_eq!(
            v.widgets.inactive.corner_radius,
            egui::CornerRadius::same(Style::RADIUS_S as u8),
            "controls take the small radius"
        );
        assert_eq!(
            v.menu_corner_radius,
            egui::CornerRadius::same(Style::RADIUS_M as u8),
            "menus take the mid radius"
        );
        assert_eq!(
            v.window_corner_radius,
            egui::CornerRadius::same(Style::RADIUS_L as u8),
            "windows/dialogs take the large radius"
        );
        assert_eq!(v.popup_shadow, Elevation::Overlay.egui_shadow());
        assert_eq!(v.window_shadow, Elevation::Modal.egui_shadow());
    }
}
