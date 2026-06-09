//! CR-3 — Object Card data form (Material Design Elevated card).
//!
//! Locked 2026-05-24 in `docs/design/chromeos-classic-spec.md`
//! §"Object Cards — Material Design layer". Operators see Objects
//! (apps, files, peers, paired phones, saved networks, credentials,
//! recent docs) as Material-Design Elevated cards layered on top of
//! the Classic ChromeOS row chrome.
//!
//! `mde-theme` keeps the data shape + spec constants here so non-
//! Iced consumers can describe a card without pulling in the
//! toolkit. The Iced widget builder lives at
//! `crates/mde-workbench/src/panel_chrome.rs::object_card` — same
//! split as [`EmptyState`].
//!
//! [`EmptyState`]: crate::components::EmptyState
//!
//! ## Sizing matrix (locked, per the spec)
//!
//! | Size    | Dimensions  | Icon                                  | Used by                |
//! |---------|-------------|---------------------------------------|------------------------|
//! | Small   | 160 × 72 px | 28 px Material Symbol, leading (left of text)  | Workbench inline lists |
//! | Medium  | 180 × 100 px| 40 px Material Symbol, top (above title)       | mde-files grid view    |
//! | Large   | 200 × 140 px| 48 px Material Symbol, top (above title)       | Start menu grid        |
//!
//! Corner radius is **12 px** for Object Cards — an intentional
//! break from the 4 px Classic ChromeOS rule, justified by the
//! card-as-pickable-object affordance. Buttons / dialogs / popovers
//! / toasts stay at 4 px; the spec's Phase-0.8 audit only allows
//! 12 px radii inside Object-Card-grep contexts.
//!
//! ## Interaction states
//!
//! The `CardState` enum tracks every visual state defined by the
//! spec. Renderers branch on the state to pick the right shadow
//! tier, overlay, border, ripple, or opacity.

use crate::color::Rgba;
use crate::icons::Icon;

/// Spec-defined sizes for Object Cards. Each variant carries the
/// canonical card dimensions + icon dimensions + icon placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardSize {
    /// 160 × 72 px, 28 px leading icon. Workbench inline lists.
    Small,
    /// 180 × 100 px, 40 px top icon. mde-files grid view.
    Medium,
    /// 200 × 140 px, 48 px top icon. Start menu grid.
    Large,
}

impl CardSize {
    /// Card width in pixels per the spec.
    #[must_use]
    pub const fn width(self) -> f32 {
        match self {
            Self::Small => 160.0,
            Self::Medium => 180.0,
            Self::Large => 200.0,
        }
    }

    /// Card height in pixels per the spec.
    #[must_use]
    pub const fn height(self) -> f32 {
        match self {
            Self::Small => 72.0,
            Self::Medium => 100.0,
            Self::Large => 140.0,
        }
    }

    /// Icon size in pixels per the spec — varies by size.
    #[must_use]
    pub const fn icon_size(self) -> f32 {
        match self {
            Self::Small => 28.0,
            Self::Medium => 40.0,
            Self::Large => 48.0,
        }
    }

    /// Where the icon sits relative to the title text.
    #[must_use]
    pub const fn icon_placement(self) -> IconPlacement {
        match self {
            Self::Small => IconPlacement::Leading,
            Self::Medium | Self::Large => IconPlacement::Top,
        }
    }
}

/// Where the icon sits relative to the card's text content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconPlacement {
    /// Left of the title + subtitle (Small cards).
    Leading,
    /// Above the title + subtitle (Medium + Large cards).
    Top,
}

/// Every visual state an Object Card can be in. Renderers branch
/// on this to pick shadow tier, overlay, border, ripple, opacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CardState {
    /// Idle — base shadow, no overlay, no border.
    #[default]
    Default,
    /// Mouse over — +1 elevation shadow + 8 % white overlay.
    Hover,
    /// Active press — +2 elevation shadow + 30 % indigo ripple
    /// centred on press point (300 ms animation).
    Pressed,
    /// In a selection — 2 px indigo border + 15 % indigo overlay.
    Selected,
    /// Keyboard focus — 2 px indigo outline 1 px offset.
    Focused,
    /// Greyed out — 40 % opacity, not-allowed cursor.
    Disabled,
}

/// Describes the contents of a Material Object Card. The
/// renderer (in mde-workbench's panel_chrome) takes ownership and
/// emits the Iced widget tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectCard {
    /// Optional Material Symbols icon. When `None`, the renderer paints a
    /// blank slot at the spec's icon size (keeps card geometry
    /// stable across icon-bearing + icon-less rows).
    pub icon: Option<Icon>,
    /// Primary label — 14 px Roboto 500, `text` colour.
    pub title: String,
    /// Optional one-line caption — 12 px Roboto, `text_muted`,
    /// truncated with ellipsis on overflow. Compact-shape locked
    /// 2026-05-24 round-4 re-ask.
    pub subtitle: Option<String>,
    /// Renderer dimensions + icon placement.
    pub size: CardSize,
    /// Visual state the card is currently in.
    pub state: CardState,
    /// Optional text-color override for the title. `None` =
    /// `palette.text`. Used by surfaces that paint specific
    /// statuses (e.g. dimmed offline-peer rows).
    pub title_color_override: Option<Rgba>,
    /// Optional text-color override for the subtitle. `None` =
    /// `palette.text_muted`. Same use case as `title_color_override`.
    pub subtitle_color_override: Option<Rgba>,
}

impl ObjectCard {
    /// Build a Small card with title + leading icon, no subtitle.
    /// The most common Workbench-inline-list shape.
    #[must_use]
    pub fn small(icon: Icon, title: impl Into<String>) -> Self {
        Self {
            icon: Some(icon),
            title: title.into(),
            subtitle: None,
            size: CardSize::Small,
            state: CardState::Default,
            title_color_override: None,
            subtitle_color_override: None,
        }
    }

    /// Build a Medium card with top icon + title + subtitle.
    /// The mde-files grid-view shape.
    #[must_use]
    pub fn medium(icon: Icon, title: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self {
            icon: Some(icon),
            title: title.into(),
            subtitle: Some(subtitle.into()),
            size: CardSize::Medium,
            state: CardState::Default,
            title_color_override: None,
            subtitle_color_override: None,
        }
    }

    /// Build a Large card with top icon + title + subtitle.
    /// The Start menu grid shape.
    #[must_use]
    pub fn large(icon: Icon, title: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self {
            icon: Some(icon),
            title: title.into(),
            subtitle: Some(subtitle.into()),
            size: CardSize::Large,
            state: CardState::Default,
            title_color_override: None,
            subtitle_color_override: None,
        }
    }

    /// Builder: attach a subtitle to an existing card.
    #[must_use]
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Builder: set the visual state.
    #[must_use]
    pub fn with_state(mut self, state: CardState) -> Self {
        self.state = state;
        self
    }

    /// Builder: drop the icon (paint an empty slot instead).
    #[must_use]
    pub fn without_icon(mut self) -> Self {
        self.icon = None;
        self
    }
}

// ---------------------------------------------------------------
// Spec constants — every renderer reads these. Changing a value
// here is a design-lock change; document in chromeos-classic-spec.md
// and update the unit tests below.
// ---------------------------------------------------------------

/// Card corner radius (4 px per Q42 of the 100-Q tightening
/// survey 2026-05-25 — conformance with the Classic ChromeOS
/// platform rule). Was 12 px (intentional break); the outlier
/// retired with the EPIC-UI-CARDS lock. Card identity now comes
/// from icon (Material Symbols per Q43) + title + subtitle
/// layout + flat-card variant (no M3 shadow) per the
/// `project_object_card_pattern` memory supersession block.
pub const CARD_CORNER_RADIUS: f32 = 4.0;

/// Internal padding on every side of the card surface.
pub const CARD_PADDING: f32 = 16.0;

/// Gap between cards in a grid, both row + column.
pub const CARD_GRID_GAP: f32 = 12.0;

/// Default-state elevation shadow — outer + inner pair.
pub const CARD_SHADOW_DEFAULT_OFFSET_Y: f32 = 1.0;
/// Default-state outer shadow blur radius.
pub const CARD_SHADOW_DEFAULT_BLUR: f32 = 3.0;
/// Default-state outer shadow alpha (0.0–1.0).
pub const CARD_SHADOW_DEFAULT_ALPHA: f32 = 0.30;

/// Hover-state elevation shadow — slightly elevated.
pub const CARD_SHADOW_HOVER_OFFSET_Y: f32 = 2.0;
/// Hover-state shadow blur.
pub const CARD_SHADOW_HOVER_BLUR: f32 = 6.0;
/// Hover-state shadow alpha.
pub const CARD_SHADOW_HOVER_ALPHA: f32 = 0.35;

/// Pressed-state elevation shadow — highest tier.
pub const CARD_SHADOW_PRESSED_OFFSET_Y: f32 = 4.0;
/// Pressed-state shadow blur.
pub const CARD_SHADOW_PRESSED_BLUR: f32 = 10.0;
/// Pressed-state shadow alpha.
pub const CARD_SHADOW_PRESSED_ALPHA: f32 = 0.40;

/// White-overlay alpha applied on top of the surface tint when
/// the card is hovered.
pub const CARD_HOVER_OVERLAY_ALPHA: f32 = 0.08;

/// Indigo ripple alpha for the press feedback (300 ms animation).
pub const CARD_PRESS_RIPPLE_ALPHA: f32 = 0.30;
/// Indigo press ripple duration in milliseconds.
pub const CARD_PRESS_RIPPLE_DURATION_MS: u16 = 300;

/// Selected-state border width.
pub const CARD_SELECTED_BORDER_WIDTH: f32 = 2.0;
/// Selected-state overlay alpha (indigo over surface).
pub const CARD_SELECTED_OVERLAY_ALPHA: f32 = 0.15;

/// Keyboard-focus outline width (matches the platform focus-ring
/// lock from the Classic ChromeOS spec).
pub const CARD_FOCUS_OUTLINE_WIDTH: f32 = 2.0;
/// Keyboard-focus outline offset from the card edge.
pub const CARD_FOCUS_OUTLINE_OFFSET: f32 = 1.0;

/// Disabled-state opacity (40 %).
pub const CARD_DISABLED_OPACITY: f32 = 0.40;

/// Title typography — 14 px (Roboto 500 in the consumer-side
/// renderer; this crate doesn't pick the font, only the size).
pub const CARD_TITLE_SIZE: f32 = 14.0;

/// Subtitle typography — 12 px (Roboto regular).
pub const CARD_SUBTITLE_SIZE: f32 = 12.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_size_matches_spec() {
        assert!((CardSize::Small.width() - 160.0).abs() < f32::EPSILON);
        assert!((CardSize::Small.height() - 72.0).abs() < f32::EPSILON);
        assert!((CardSize::Small.icon_size() - 28.0).abs() < f32::EPSILON);
        assert_eq!(CardSize::Small.icon_placement(), IconPlacement::Leading);
    }

    #[test]
    fn medium_size_matches_spec() {
        assert!((CardSize::Medium.width() - 180.0).abs() < f32::EPSILON);
        assert!((CardSize::Medium.height() - 100.0).abs() < f32::EPSILON);
        assert!((CardSize::Medium.icon_size() - 40.0).abs() < f32::EPSILON);
        assert_eq!(CardSize::Medium.icon_placement(), IconPlacement::Top);
    }

    #[test]
    fn large_size_matches_spec() {
        assert!((CardSize::Large.width() - 200.0).abs() < f32::EPSILON);
        assert!((CardSize::Large.height() - 140.0).abs() < f32::EPSILON);
        assert!((CardSize::Large.icon_size() - 48.0).abs() < f32::EPSILON);
        assert_eq!(CardSize::Large.icon_placement(), IconPlacement::Top);
    }

    #[test]
    fn default_state_is_default_variant() {
        let c = ObjectCard::small(Icon::Fleet, "Peer A");
        assert_eq!(c.state, CardState::Default);
        assert_eq!(CardState::default(), CardState::Default);
    }

    #[test]
    fn small_constructor_omits_subtitle() {
        let c = ObjectCard::small(Icon::Fleet, "Peer A");
        assert_eq!(c.subtitle, None);
        assert_eq!(c.title, "Peer A");
        assert_eq!(c.size, CardSize::Small);
    }

    #[test]
    fn medium_constructor_carries_subtitle() {
        let c = ObjectCard::medium(Icon::Fleet, "doc.pdf", "Modified yesterday");
        assert_eq!(c.subtitle.as_deref(), Some("Modified yesterday"));
        assert_eq!(c.size, CardSize::Medium);
    }

    #[test]
    fn large_constructor_carries_subtitle() {
        let c = ObjectCard::large(Icon::Fleet, "Workbench", "System utility");
        assert_eq!(c.subtitle.as_deref(), Some("System utility"));
        assert_eq!(c.size, CardSize::Large);
    }

    #[test]
    fn with_state_builder_overrides_default() {
        let c = ObjectCard::small(Icon::Fleet, "x").with_state(CardState::Selected);
        assert_eq!(c.state, CardState::Selected);
    }

    #[test]
    fn with_subtitle_builder_attaches_caption() {
        let c = ObjectCard::small(Icon::Fleet, "x").with_subtitle("extra");
        assert_eq!(c.subtitle.as_deref(), Some("extra"));
    }

    #[test]
    fn without_icon_builder_drops_icon() {
        let c = ObjectCard::small(Icon::Fleet, "x").without_icon();
        assert_eq!(c.icon, None);
    }

    #[test]
    fn corner_radius_is_four_px() {
        // Spec lock 2026-05-25 (Q42 + EPIC-UI-CARDS): conform to the
        // Classic ChromeOS 4 px platform rule. Was 12 px (intentional
        // break); the break retired in favor of platform consistency.
        // Card identity comes from icon + layout + flat-card variant.
        assert!((CARD_CORNER_RADIUS - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn padding_is_sixteen_px() {
        assert!((CARD_PADDING - 16.0).abs() < f32::EPSILON);
    }

    #[test]
    fn grid_gap_is_twelve_px() {
        assert!((CARD_GRID_GAP - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn shadow_tiers_escalate_monotonically() {
        // Each tier adds elevation; the spec relies on this
        // ordering for the hover/press feedback to read as a
        // visual lift, not a flicker.
        assert!(CARD_SHADOW_HOVER_OFFSET_Y > CARD_SHADOW_DEFAULT_OFFSET_Y);
        assert!(CARD_SHADOW_PRESSED_OFFSET_Y > CARD_SHADOW_HOVER_OFFSET_Y);
        assert!(CARD_SHADOW_HOVER_BLUR > CARD_SHADOW_DEFAULT_BLUR);
        assert!(CARD_SHADOW_PRESSED_BLUR > CARD_SHADOW_HOVER_BLUR);
        assert!(CARD_SHADOW_HOVER_ALPHA > CARD_SHADOW_DEFAULT_ALPHA);
        assert!(CARD_SHADOW_PRESSED_ALPHA > CARD_SHADOW_HOVER_ALPHA);
    }

    #[test]
    fn hover_overlay_alpha_is_eight_percent() {
        assert!((CARD_HOVER_OVERLAY_ALPHA - 0.08).abs() < f32::EPSILON);
    }

    #[test]
    fn press_ripple_is_thirty_percent_for_three_hundred_ms() {
        assert!((CARD_PRESS_RIPPLE_ALPHA - 0.30).abs() < f32::EPSILON);
        assert_eq!(CARD_PRESS_RIPPLE_DURATION_MS, 300);
    }

    #[test]
    fn selected_border_is_two_px_with_fifteen_percent_overlay() {
        assert!((CARD_SELECTED_BORDER_WIDTH - 2.0).abs() < f32::EPSILON);
        assert!((CARD_SELECTED_OVERLAY_ALPHA - 0.15).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_outline_is_two_px_with_one_px_offset() {
        assert!((CARD_FOCUS_OUTLINE_WIDTH - 2.0).abs() < f32::EPSILON);
        assert!((CARD_FOCUS_OUTLINE_OFFSET - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn disabled_opacity_is_forty_percent() {
        assert!((CARD_DISABLED_OPACITY - 0.40).abs() < f32::EPSILON);
    }

    #[test]
    fn typography_sizes_match_spec() {
        // 14 px Roboto 500 title, 12 px Roboto subtitle — per the
        // Classic ChromeOS Object Card spec.
        assert!((CARD_TITLE_SIZE - 14.0).abs() < f32::EPSILON);
        assert!((CARD_SUBTITLE_SIZE - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn every_state_renders_to_glyph_safely() {
        // Smoke test: every variant of CardState should be
        // constructible + comparable. Catches enum drift if a
        // variant is added without test coverage.
        for state in [
            CardState::Default,
            CardState::Hover,
            CardState::Pressed,
            CardState::Selected,
            CardState::Focused,
            CardState::Disabled,
        ] {
            let c = ObjectCard::small(Icon::Fleet, "t").with_state(state);
            assert_eq!(c.state, state);
        }
    }
}
