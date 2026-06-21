//! # MDE Design System
//!
//! The Rust-native design token surface for Mackes Desktop
//! Environment. Lock authority: `docs/design/visual-identity.md`
//! and `docs/PROJECT_WORKLIST.md` § UX Design Locks (50-Q survey,
//! 2026-05-21).
//!
//! ## Surface
//!
//! - [`color::Rgba`] — primitive RGBA color (no Iced runtime dep
//!   in the default build).
//! - [`palette`] — named color tokens for dark + light themes.
//! - [`spacing`] — the 12-step modular spacing scale (NFU-1).
//! - [`typography`] — type-scale sizes + font-stack constants.
//! - [`radii`] — corner-radius tokens.
//! - [`shadows`] — elevation shadow specs.
//! - [`Theme`] — Dark / Light enum.
//! - [`Density`] — Compact / Comfortable / Spacious enum
//!   (UX-15). UX-24 sub-lock: density scales spacing tokens only,
//!   never component dimensions.
//! - [`Tokens`] — resolved token set for a given (theme, density)
//!   pair. The single struct every consumer reads.
//! - [`Brand`] — runtime brand-asset loader. Maps logical slots
//!   (wordmark, monogram, app icon, greeter art) to bytes, with
//!   a `$MDE_BRAND_DIR` override layer and `include_bytes!`
//!   fallbacks. See `assets/brand/README.md` for the slot table
//!   and replacement workflow.
//!
//! ## Iced interop
//!
//! Behind the `iced` feature flag, this crate adds conversion
//! helpers (`Rgba::into_iced_color()`, `FontSize::px()`, etc.) so
//! Iced views can consume tokens directly. Without the feature
//! the crate is dependency-free and unit-testable in isolation.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod accessibility;
pub mod animation;
pub mod brand;
pub mod carbon;
pub mod color;
pub mod components;
pub mod density;
// AUD2-3 (2026-06-12): `elevation` removed — the Q29/Q30 per-tier
// radius+shadow bundles targeted the deleted shell's OSD/greeter/menu
// surfaces, which Cosmic owns post-E11; zero workspace callers. The
// shadow specs live on in `shadows` (consumed via `Theme::modal_shadow`).
pub mod hero;
pub mod icons;
pub mod load_state;
pub mod motion;
pub mod palette;
pub mod prefs;
pub mod radii;
pub mod shadows;
pub mod spacing;
pub mod theme;
pub mod typography;

pub use accessibility::A11y;
pub use animation::{
    crossfade, ease, fade_in, fade_out, lerp_f32, lift_on_hover, pulse_scale, slide_in, Animator,
    Crossfade, LoopingTween, RenderParams, Transition, Tween,
};
pub use brand::{Brand, BrandAsset, BrandFormat, BrandSlot, BrandSource};
pub use color::Rgba;
pub use components::{
    CardSize, CardState, EmptyState, IconPlacement, ObjectCard, CARD_CORNER_RADIUS,
    CARD_DISABLED_OPACITY, CARD_FOCUS_OUTLINE_OFFSET, CARD_FOCUS_OUTLINE_WIDTH, CARD_GRID_GAP,
    CARD_HOVER_OVERLAY_ALPHA, CARD_PADDING, CARD_PRESS_RIPPLE_ALPHA, CARD_PRESS_RIPPLE_DURATION_MS,
    CARD_SELECTED_BORDER_WIDTH, CARD_SELECTED_OVERLAY_ALPHA, CARD_SHADOW_DEFAULT_ALPHA,
    CARD_SHADOW_DEFAULT_BLUR, CARD_SHADOW_DEFAULT_OFFSET_Y, CARD_SHADOW_HOVER_ALPHA,
    CARD_SHADOW_HOVER_BLUR, CARD_SHADOW_HOVER_OFFSET_Y, CARD_SHADOW_PRESSED_ALPHA,
    CARD_SHADOW_PRESSED_BLUR, CARD_SHADOW_PRESSED_OFFSET_Y, CARD_SUBTITLE_SIZE, CARD_TITLE_SIZE,
};
pub use density::Density;
pub use icons::{
    icon_for_device_type, mde_icon, FillMode, Icon, IconSize, IconState, ResolvedIcon,
    MATERIAL_LINE_WEIGHT_PX,
};
pub use load_state::{LoadState, StatusSeverity};
pub use motion::{Easing, Motion, PANEL_MOUNT_TRANSLATE_Y_PX, PULSE_MAX_SCALE};
pub use palette::Palette;
pub use prefs::Preferences;
pub use radii::Radii;
pub use shadows::Shadow;
pub use spacing::Space;
pub use theme::{Theme, Tokens};
pub use typography::{FontSize, FontWeight, LetterSpacing, TypeRole};

/// Convenience: resolved tokens for the most common case
/// (dark theme + comfortable density). Use in tests, demos, and
/// any surface where the user hasn't expressed a preference yet.
pub fn default_tokens() -> Tokens {
    Tokens::resolve(Theme::Dark, Density::Comfortable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resolves_without_panic() {
        let t = default_tokens();
        assert_eq!(t.theme, Theme::Dark);
        assert_eq!(t.density, Density::Comfortable);
    }

    #[test]
    fn density_modes_resolve_distinct_scaled_spacings() {
        let c = Tokens::resolve(Theme::Dark, Density::Compact);
        let m = Tokens::resolve(Theme::Dark, Density::Comfortable);
        let s = Tokens::resolve(Theme::Dark, Density::Spacious);
        // UX-24: density scales spacings; component dimensions
        // (nav row, button height) are NOT density-scaled.
        assert!(c.space.md < m.space.md);
        assert!(m.space.md < s.space.md);
    }

    #[test]
    fn both_themes_resolve_with_full_palette() {
        let d = Tokens::resolve(Theme::Dark, Density::Comfortable);
        let l = Tokens::resolve(Theme::Light, Density::Comfortable);
        // E9 (2026-06-07): Carbon uses one uniform interactive blue
        // (Blue 60) across both gray themes — the ChromeOS per-theme
        // accent shift is retired.
        assert_eq!(d.palette.accent, l.palette.accent);
        let da = d.palette.accent;
        assert!(da.b > da.r && da.b > da.g, "accent reads as Carbon blue");
        // Background diverges between themes (Gray 100 vs Gray 10).
        assert_ne!(d.palette.background, l.palette.background);
    }
}
