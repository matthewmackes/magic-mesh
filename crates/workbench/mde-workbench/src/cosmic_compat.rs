//! CUT-1 — libcosmic compat shims for the Workbench port. Bridges the
//! iced-style per-widget style closures the panels were written against to
//! libcosmic's class-based theming, plus a local IntoIcedColor (replacing the
//! mde-theme "iced" feature) and a local object_card (replacing mde-iced-components).
use cosmic::iced::widget::{button, container, svg};
use cosmic::iced::widget::{Button, Container, Svg, Text};
use cosmic::iced::Color;
use cosmic::Theme;

pub trait ContainerSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self;
}
impl<'a, M: 'a> ContainerSty<'a, M> for Container<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self {
        self.style(f)
    }
}

pub trait ButtonSty<'a, M: 'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self;
}
impl<'a, M: 'a> ButtonSty<'a, M> for Button<'a, M, Theme> {
    fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Button::Custom(Box::new(f)))
    }
}

pub trait SvgSty<'a> {
    #[must_use]
    fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self;
}
impl<'a> SvgSty<'a> for Svg<'a, Theme> {
    fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self {
        self.class(cosmic::theme::iced::Svg::custom(f))
    }
}

pub trait TextSty<'a> {
    #[must_use]
    fn colr(self, color: impl Into<Color>) -> Self;
}
impl<'a> TextSty<'a> for Text<'a, Theme> {
    fn colr(self, color: impl Into<Color>) -> Self {
        self.class(cosmic::theme::iced::Text::Color(color.into()))
    }
}

// ---------------------------------------------------------------------------
// (a) IntoIcedColor — replaces the dropped mde-theme "iced" feature. The
// panels call `x.into_cosmic_color()` (~632 sites); this extension trait keeps
// the exact method name so those sites compile unchanged. Rgba is Copy, so
// the by-value impl covers `&Rgba` derefs too, but we add the reference impl
// for sites that hold a borrow.
// ---------------------------------------------------------------------------

/// Convert an `mde_theme::Rgba` token into a `cosmic::iced::Color`. Reprovides
/// the method name the panels were written against before the mde-theme
/// "iced" feature was dropped for the fork cutover.
pub trait IntoIcedColor {
    /// Map the 8-bit RGB + f32 alpha token into a normalized iced color.
    fn into_cosmic_color(self) -> Color;
}
impl IntoIcedColor for mde_theme::Rgba {
    fn into_cosmic_color(self) -> Color {
        Color {
            r: self.r as f32 / 255.0,
            g: self.g as f32 / 255.0,
            b: self.b as f32 / 255.0,
            a: self.a,
        }
    }
}
impl IntoIcedColor for &mde_theme::Rgba {
    fn into_cosmic_color(self) -> Color {
        (*self).into_cosmic_color()
    }
}

// ---------------------------------------------------------------------------
// (b) object_card — ported from mde-iced-components/src/lib.rs. Every `iced::`
// is rewritten to `cosmic::iced::` and the return type carries the explicit
// `Theme` generic so the element threads libcosmic's theme through the tree.
// ---------------------------------------------------------------------------

use cosmic::iced::widget::{column, container as container_fn, row, text, Column, Space};
use cosmic::iced::{alignment, Background, Border, Element, Length, Padding, Shadow as IcedShadow};

pub use mde_theme::ObjectCard;
use mde_theme::{
    mde_icon, CardSize, CardState, IconPlacement, IconSize, IconState, Palette, CARD_CORNER_RADIUS,
    CARD_DISABLED_OPACITY, CARD_FOCUS_OUTLINE_OFFSET, CARD_FOCUS_OUTLINE_WIDTH,
    CARD_HOVER_OVERLAY_ALPHA, CARD_PADDING, CARD_SELECTED_BORDER_WIDTH,
    CARD_SELECTED_OVERLAY_ALPHA, CARD_SHADOW_DEFAULT_ALPHA, CARD_SHADOW_DEFAULT_BLUR,
    CARD_SHADOW_DEFAULT_OFFSET_Y, CARD_SHADOW_HOVER_ALPHA, CARD_SHADOW_HOVER_BLUR,
    CARD_SHADOW_HOVER_OFFSET_Y, CARD_SHADOW_PRESSED_ALPHA, CARD_SHADOW_PRESSED_BLUR,
    CARD_SHADOW_PRESSED_OFFSET_Y, CARD_SUBTITLE_SIZE, CARD_TITLE_SIZE,
};

/// CR-3 — Material Design Elevated Object Card renderer.
///
/// Takes ownership of an `ObjectCard` data form (built via
/// `ObjectCard::small/medium/large(...)`) + the active palette, returns the
/// rendered libcosmic element. The data form lives in `mde_theme` so panel
/// authors can describe an object without pulling iced; this fn is the
/// canonical render path so every Object surface shares one component.
///
/// State branches:
///   * `Default`  — base shadow, no overlay, no border.
///   * `Hover`    — +1 elevation shadow, 8 % white overlay.
///   * `Pressed`  — +2 elevation shadow.
///   * `Selected` — 2 px indigo border + 15 % indigo overlay.
///   * `Focused`  — 2 px indigo outline at 1 px offset.
///   * `Disabled` — 40 % opacity, no hover affordance.
pub fn object_card<'a, Message: 'a>(
    card: ObjectCard,
    palette: Palette,
) -> Element<'a, Message, Theme> {
    let title_color = card
        .title_color_override
        .unwrap_or(palette.text)
        .into_cosmic_color();
    let subtitle_color = card
        .subtitle_color_override
        .unwrap_or(palette.text_muted)
        .into_cosmic_color();
    let accent_color = palette.accent.into_cosmic_color();
    let card_size = card.size;
    let card_state = card.state;

    // ---- icon slot ---------------------------------------------
    let icon_slot: Element<'a, Message, Theme> = if let Some(icon) = card.icon {
        let icon_px = card_size.icon_size();
        let tier = match card_size {
            CardSize::Small => IconSize::Nav,
            CardSize::Medium | CardSize::Large => IconSize::EmptyState,
        };
        let icon_state = match card_state {
            CardState::Selected => IconState::Active,
            _ => IconState::Idle,
        };
        let resolved = mde_icon(icon, tier);
        let svg_bytes = resolved.svg_bytes_for_state(icon_state);
        use cosmic::iced::widget::svg as widget_svg;
        let muted = palette.text.into_cosmic_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(icon_px))
            .height(Length::Fixed(icon_px))
            .class(cosmic::theme::iced::Svg::custom(move |_t: &Theme| {
                widget_svg::Style { color: Some(muted) }
            }))
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(card_size.icon_size()))
            .height(Length::Fixed(card_size.icon_size()))
            .into()
    };

    // ---- title + subtitle column -------------------------------
    let title_widget = text(card.title)
        .size(CARD_TITLE_SIZE)
        .class(cosmic::theme::iced::Text::Color(title_color));

    let text_col: Column<'a, Message, Theme> = if let Some(subtitle) = card.subtitle {
        column![
            title_widget,
            text(subtitle)
                .size(CARD_SUBTITLE_SIZE)
                .class(cosmic::theme::iced::Text::Color(subtitle_color)),
        ]
        .spacing(2)
    } else {
        column![title_widget]
    };

    // ---- content layout (leading vs top icon) ------------------
    let content: Element<'a, Message, Theme> = match card_size.icon_placement() {
        IconPlacement::Leading => row![icon_slot, text_col]
            .spacing(12)
            .align_y(alignment::Vertical::Center)
            .into(),
        IconPlacement::Top => column![icon_slot, text_col]
            .spacing(8)
            .align_x(alignment::Horizontal::Center)
            .into(),
    };

    // ---- per-state visual params -------------------------------
    let (shadow_offset, shadow_blur, shadow_alpha) = match card_state {
        CardState::Hover => (
            CARD_SHADOW_HOVER_OFFSET_Y,
            CARD_SHADOW_HOVER_BLUR,
            CARD_SHADOW_HOVER_ALPHA,
        ),
        CardState::Pressed => (
            CARD_SHADOW_PRESSED_OFFSET_Y,
            CARD_SHADOW_PRESSED_BLUR,
            CARD_SHADOW_PRESSED_ALPHA,
        ),
        _ => (
            CARD_SHADOW_DEFAULT_OFFSET_Y,
            CARD_SHADOW_DEFAULT_BLUR,
            CARD_SHADOW_DEFAULT_ALPHA,
        ),
    };

    let bg = match card_state {
        CardState::Hover => overlay_white_on(palette.surface, CARD_HOVER_OVERLAY_ALPHA),
        CardState::Selected => {
            overlay_color_on(palette.surface, accent_color, CARD_SELECTED_OVERLAY_ALPHA)
        }
        _ => palette.surface.into_cosmic_color(),
    };

    let border = match card_state {
        CardState::Selected => Border {
            color: accent_color,
            width: CARD_SELECTED_BORDER_WIDTH,
            radius: CARD_CORNER_RADIUS.into(),
        },
        CardState::Focused => Border {
            color: accent_color,
            width: CARD_FOCUS_OUTLINE_WIDTH,
            radius: (CARD_CORNER_RADIUS + CARD_FOCUS_OUTLINE_OFFSET).into(),
        },
        _ => Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: CARD_CORNER_RADIUS.into(),
        },
    };

    let final_bg = if matches!(card_state, CardState::Disabled) {
        with_alpha(bg, CARD_DISABLED_OPACITY)
    } else {
        bg
    };

    container_fn(content)
        .width(Length::Fixed(card_size.width()))
        .height(Length::Fixed(card_size.height()))
        .padding(Padding {
            top: CARD_PADDING,
            right: CARD_PADDING,
            bottom: CARD_PADDING,
            left: CARD_PADDING,
        })
        .style(move |_theme: &Theme| container::Style {
            icon_color: None,
            background: Some(Background::Color(final_bg)),
            border,
            shadow: IcedShadow {
                // Pure-black scrim token; the elevation alpha is the dynamic part.
                color: Color {
                    a: shadow_alpha,
                    ..mde_theme::carbon::BLACK.into_cosmic_color()
                },
                offset: cosmic::iced::Vector::new(0.0, shadow_offset),
                blur_radius: shadow_blur,
            },
            text_color: Some(title_color),
            snap: false,
        })
        .into()
}

/// Helper: paint a white overlay at the given alpha on top of a surface token.
pub fn overlay_white_on(base: mde_theme::Rgba, alpha: f32) -> Color {
    let base_iced = base.into_cosmic_color();
    Color {
        r: lerp(base_iced.r, 1.0, alpha),
        g: lerp(base_iced.g, 1.0, alpha),
        b: lerp(base_iced.b, 1.0, alpha),
        a: base_iced.a,
    }
}

/// Helper: paint a coloured overlay at the given alpha on top of a surface token.
pub fn overlay_color_on(base: mde_theme::Rgba, overlay: Color, alpha: f32) -> Color {
    let base_iced = base.into_cosmic_color();
    Color {
        r: lerp(base_iced.r, overlay.r, alpha),
        g: lerp(base_iced.g, overlay.g, alpha),
        b: lerp(base_iced.b, overlay.b, alpha),
        a: base_iced.a,
    }
}

/// Helper: multiply a colour's alpha by `mul`. Used for the disabled state's
/// 40 % opacity rule.
pub fn with_alpha(c: Color, mul: f32) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a * mul,
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------
// (c) prelude — transform agents add one `use cosmic_compat::prelude::*;` per
// file to pick up the style-closure traits, IntoIcedColor, and object_card.
// ---------------------------------------------------------------------------

pub mod prelude {
    pub use super::{
        object_card, ButtonSty, ContainerSty, IntoIcedColor, ObjectCard, SvgSty, TextSty,
    };
}
