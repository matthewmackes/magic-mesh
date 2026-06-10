//! CR-9 (supersedes UX-7) — Classic ChromeOS form-control tokens.
//!
//! Centralizes the three button variants, styled text input,
//! toggle pill, skeleton placeholder, spinner, checkbox style,
//! radio style, and scrollbar style so every Iced panel renders
//! consistent hover / focus / active / disabled states.
//!
//! Token rules (CR-9 spec, docs/design/chromeos-classic-spec.md
//! §Primary button / §Text input / §Toggle/checkbox/radio /
//! §Scrollbar / §Focus ring):
//!   * buttons: 32 px height, 4 px corners, 16 px H pad, 3 variants
//!   * text inputs: 32 px height, sharp corners, transparent bg,
//!     palette.border bottom line (Iced renders full border; bottom-
//!     only requires canvas — deferred to UX-9.a)
//!   * toggles: 32×16 px pill, 12 px knob, palette.border off bg
//!   * checkbox: 16 px sharp square, accent fill + white check
//!   * radio: 16 px circle, transparent bg, accent dot on select
//!   * scrollbar: 12 px always-visible, surface track, border thumb
//!   * focus ring: 2 px indigo, 1 px offset

use iced::widget::button::Status as ButtonStatus;
use iced::widget::checkbox::{Status as CheckboxStatus, Style as CheckboxStyle};
use iced::widget::radio::{Status as RadioStatus, Style as RadioStyle};
use iced::widget::scrollable::{Rail, Scroller, Status as ScrollStatus, Style as ScrollStyle};
use iced::widget::{button, container, row, text, text_input, Space};
use iced::{alignment, Background, Border, Color, Element, Length, Padding, Shadow};

use mde_theme::{FontSize, Palette, Radii, TypeRole};

/// CR-9 — button height. 32 px per Classic ChromeOS spec.
pub const BUTTON_HEIGHT: f32 = 32.0;

/// CR-9 — button horizontal padding. 16 px per Classic ChromeOS spec.
pub const BUTTON_HORIZONTAL_PADDING: f32 = 16.0;

/// CR-9 — focus ring width on focused buttons / inputs. 2 px.
pub const FOCUS_RING_WIDTH: f32 = 2.0;

/// CR-9 — focus ring offset. 1 px per Classic ChromeOS spec.
pub const FOCUS_RING_OFFSET: f32 = 1.0;

/// CR-9 — text input height. 32 px per Classic ChromeOS spec.
pub const INPUT_HEIGHT: f32 = 32.0;

/// CR-9 — toggle pill dimensions. 32×16 px per Classic ChromeOS spec.
pub const TOGGLE_WIDTH: f32 = 32.0;
pub const TOGGLE_HEIGHT: f32 = 16.0;

/// CR-9 — toggle knob diameter. 12 px circle per Classic ChromeOS spec.
pub const TOGGLE_KNOB_DIAMETER: f32 = 12.0;

/// CR-9 — scrollbar rail width. 12 px always-visible per spec.
/// Set on the widget via `scrollable::Scrollbar::new().width(SCROLLBAR_WIDTH)`;
/// `scrollbar_style()` controls the colors only.
pub const SCROLLBAR_WIDTH: f32 = 12.0;

/// Disabled opacity multiplier.
pub const DISABLED_OPACITY: f32 = 0.40;

/// Button variants (unchanged from UX-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Accent-fill — primary action. White text, indigo background.
    Primary,
    /// 1 px outline — secondary action. Accent border + accent text.
    Secondary,
    /// Text-only — tertiary / inline.
    Ghost,
}

/// CR-9 — render a styled button with the locked Classic ChromeOS chrome.
/// Pass `None` to `on_press` for the disabled state.
pub fn variant_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    variant: ButtonVariant,
    on_press: Option<Message>,
    palette: Palette,
) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_iced_color();
    let text_role = TypeRole::Body;
    let label_text = text(label.into())
        .size(text_role.size_in(sizes))
        .color(text_color_for_variant(variant, palette))
        .align_y(alignment::Vertical::Center);

    let style = move |_theme: &iced::Theme, status: ButtonStatus| {
        let mut bg = base_bg_for_variant(variant, accent, palette);
        let mut fg = text_color_for_variant(variant, palette);
        let mut border = border_for_variant(variant, accent, palette);
        match status {
            ButtonStatus::Hovered => bg = brighten(bg, 1.08), // +8% luminance
            ButtonStatus::Pressed => bg = brighten(bg, 0.92), // -8% luminance
            ButtonStatus::Disabled => {
                fg = with_alpha(fg, DISABLED_OPACITY);
                bg = with_alpha(bg, DISABLED_OPACITY * bg.a.max(0.1));
                border.color = with_alpha(border.color, DISABLED_OPACITY);
            }
            ButtonStatus::Active => {}
        }
        button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color: fg,
            border,
            shadow: Shadow::default(),
        }
    };

    let mut btn = button(label_text)
        .padding(Padding {
            top: 0.0,
            right: BUTTON_HORIZONTAL_PADDING,
            bottom: 0.0,
            left: BUTTON_HORIZONTAL_PADDING,
        })
        .height(Length::Fixed(BUTTON_HEIGHT))
        .style(style);
    if let Some(msg) = on_press {
        btn = btn.on_press(msg);
    }
    btn.into()
}

fn base_bg_for_variant(variant: ButtonVariant, accent: Color, _palette: Palette) -> Color {
    match variant {
        ButtonVariant::Primary => accent,
        ButtonVariant::Secondary | ButtonVariant::Ghost => Color::TRANSPARENT,
    }
}

fn text_color_for_variant(variant: ButtonVariant, palette: Palette) -> Color {
    match variant {
        ButtonVariant::Primary => Color::WHITE,
        ButtonVariant::Secondary => palette.accent.into_iced_color(),
        ButtonVariant::Ghost => palette.text.into_iced_color(),
    }
}

fn border_for_variant(variant: ButtonVariant, accent: Color, _palette: Palette) -> Border {
    let radii = Radii::defaults();
    // CR-9: 4 px corners = radii.sm
    let r = f32::from(radii.sm).into();
    match variant {
        ButtonVariant::Primary => Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: r,
        },
        ButtonVariant::Secondary => Border {
            color: accent,
            width: 1.0,
            radius: r,
        },
        ButtonVariant::Ghost => Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: r,
        },
    }
}

/// CR-9 — styled text input. Transparent bg, palette.border divider line
/// at 1 px (spec says bottom-only; Iced renders all sides — true bottom-
/// only requires canvas, deferred to UX-9.a), 2 px accent on focus.
/// 32 px height, sharp corners.
pub fn styled_text_input<'a, Message: Clone + 'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
    palette: Palette,
) -> Element<'a, Message> {
    let divider = palette.border.into_iced_color();
    let accent = palette.accent.into_iced_color();
    let muted = palette.text_muted.into_iced_color();
    let text_color = palette.text.into_iced_color();

    text_input(placeholder, value)
        .on_input(on_input)
        .padding(Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        })
        .size(13)
        .style(move |_theme, status| {
            let (border_color, border_width) = match status {
                text_input::Status::Focused { .. } => (accent, 2.0),
                _ => (divider, 1.0),
            };
            text_input::Style {
                background: Background::Color(Color::TRANSPARENT),
                border: Border {
                    color: border_color,
                    width: border_width,
                    radius: 0.0.into(),
                },
                icon: muted,
                placeholder: muted,
                value: text_color,
                selection: with_alpha(accent, 0.3),
            }
        })
        .into()
}

/// CR-9 — toggle pill. 32×16 px, 12 px knob, palette.border off bg,
/// accent on bg. Slide animation (140 ms ease-out per spec) deferred
/// to UX-9.a subscription wiring — stateless snap for now.
pub fn toggle<'a, Message: Clone + 'a>(
    value: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
    palette: Palette,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let accent = palette.accent.into_iced_color();
    let bg_off = palette.border.into_iced_color();
    let bg_on = accent;
    let knob_color = Color::WHITE;

    let on_msg = on_toggle(!value);

    let knob_offset = if value {
        TOGGLE_WIDTH - TOGGLE_HEIGHT
    } else {
        0.0
    };

    let knob = container(
        Space::new()
            .width(Length::Fixed(TOGGLE_KNOB_DIAMETER))
            .height(Length::Fixed(TOGGLE_KNOB_DIAMETER)),
    )
    .width(Length::Fixed(TOGGLE_KNOB_DIAMETER))
    .height(Length::Fixed(TOGGLE_KNOB_DIAMETER))
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(knob_color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: f32::from(radii.full).into(),
        },
        ..container::Style::default()
    });

    let pill_content = row![Space::new().width(Length::Fixed(knob_offset + 2.0)), knob,]
        .align_y(alignment::Vertical::Center)
        .height(Length::Fixed(TOGGLE_HEIGHT));

    button(pill_content)
        .padding(0)
        .width(Length::Fixed(TOGGLE_WIDTH))
        .height(Length::Fixed(TOGGLE_HEIGHT))
        .on_press(on_msg)
        .style(move |_theme, status| {
            let mut bg = if value { bg_on } else { bg_off };
            if matches!(status, ButtonStatus::Hovered) {
                bg = brighten(bg, 1.05);
            }
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::TRANSPARENT,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: f32::from(radii.full).into(),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

/// CR-9 — checkbox style closure for `iced::widget::checkbox::style()`.
/// 16 px sharp square (set via `.size(16)` on the widget), accent fill
/// when checked, white checkmark icon.
pub fn checkbox_style(palette: Palette) -> impl Fn(&iced::Theme, CheckboxStatus) -> CheckboxStyle {
    let accent = palette.accent.into_iced_color();
    let divider = palette.border.into_iced_color();
    move |_theme, status| {
        let is_checked = match status {
            CheckboxStatus::Active { is_checked }
            | CheckboxStatus::Hovered { is_checked }
            | CheckboxStatus::Disabled { is_checked } => is_checked,
        };
        let (bg, icon_color, border_color) = if is_checked {
            let fill = match status {
                CheckboxStatus::Hovered { .. } => brighten(accent, 1.08),
                CheckboxStatus::Disabled { .. } => with_alpha(accent, DISABLED_OPACITY),
                _ => accent,
            };
            (Background::Color(fill), Color::WHITE, fill)
        } else {
            let border = match status {
                CheckboxStatus::Hovered { .. } => accent,
                CheckboxStatus::Disabled { .. } => with_alpha(divider, DISABLED_OPACITY),
                _ => divider,
            };
            (
                Background::Color(Color::TRANSPARENT),
                Color::TRANSPARENT,
                border,
            )
        };
        CheckboxStyle {
            background: bg,
            icon_color,
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 0.0.into(),
            },
            text_color: None,
        }
    }
}

/// CR-9 — radio style closure for `iced::widget::radio::style()`.
/// 16 px circle (set via `.size(16)` on the widget), transparent bg,
/// accent dot when selected, palette.border ring when idle.
pub fn radio_style(palette: Palette) -> impl Fn(&iced::Theme, RadioStatus) -> RadioStyle {
    let accent = palette.accent.into_iced_color();
    let divider = palette.border.into_iced_color();
    move |_theme, status| {
        let is_selected = match status {
            RadioStatus::Active { is_selected } | RadioStatus::Hovered { is_selected } => {
                is_selected
            }
        };
        let (dot_color, border_color) = if is_selected {
            let c = match status {
                RadioStatus::Hovered { .. } => brighten(accent, 1.08),
                _ => accent,
            };
            (c, c)
        } else {
            let border = match status {
                RadioStatus::Hovered { .. } => accent,
                _ => divider,
            };
            (Color::TRANSPARENT, border)
        };
        RadioStyle {
            background: Background::Color(Color::TRANSPARENT),
            dot_color,
            border_width: 1.0,
            border_color,
            text_color: None,
        }
    }
}

/// CR-9 — scrollbar appearance closure for `scrollable::style()`.
/// Colors: palette.surface track, palette.border thumb, slight brightening
/// on hover/drag. Rail width is caller-controlled via
/// `scrollable::Scrollbar::new().width(SCROLLBAR_WIDTH)`.
pub fn scrollbar_style(palette: Palette) -> impl Fn(&iced::Theme, ScrollStatus) -> ScrollStyle {
    let track_bg = palette.surface.into_iced_color();
    let thumb_default = palette.border.into_iced_color();
    move |_theme, status| {
        let thumb_color = match status {
            ScrollStatus::Hovered {
                is_vertical_scrollbar_hovered: true,
                ..
            } => brighten(thumb_default, 1.15),
            ScrollStatus::Dragged {
                is_vertical_scrollbar_dragged: true,
                ..
            } => brighten(thumb_default, 1.25),
            _ => thumb_default,
        };
        let rail = Rail {
            background: Some(Background::Color(track_bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            scroller: Scroller {
                background: Background::Color(thumb_color),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
            },
        };
        ScrollStyle {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            // iced 0.14 added the auto-scroll overlay; render it
            // invisible (transparent) to preserve prior behavior.
            auto_scroll: iced::widget::scrollable::AutoScroll {
                background: Background::Color(Color::TRANSPARENT),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: Shadow::default(),
                icon: Color::TRANSPARENT,
            },
        }
    }
}

/// UX-7 (d) — skeleton placeholder. Accent-tinted rectangle;
/// shimmer animation wires in UX-9.a.
pub fn skeleton<'a, Message: 'a>(
    width: f32,
    height: f32,
    palette: Palette,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let bg = with_alpha(palette.raised.into_iced_color(), 0.6);
    container(
        Space::new()
            .width(Length::Fixed(width))
            .height(Length::Fixed(height)),
    )
    .width(Length::Fixed(width))
    .height(Length::Fixed(height))
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: f32::from(radii.sm).into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// UX-7 (d) — spinner placeholder. Static accent circle; animation
/// wiring deferred to UX-9.a.
pub fn spinner<'a, Message: 'a>(palette: Palette) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let accent = palette.accent.into_iced_color();
    container(
        Space::new()
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0)),
    )
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0))
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(with_alpha(accent, 0.6))),
        border: Border {
            color: accent,
            width: 1.0,
            radius: f32::from(radii.full).into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn brighten(c: Color, factor: f32) -> Color {
    Color {
        r: (c.r * factor).clamp(0.0, 1.0),
        g: (c.g * factor).clamp(0.0, 1.0),
        b: (c.b * factor).clamp(0.0, 1.0),
        a: c.a,
    }
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_height_locked_to_32() {
        assert!((BUTTON_HEIGHT - 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn input_height_matches_button_height() {
        // CR-9: inputs share button height (32 px) for visual row alignment.
        assert!((INPUT_HEIGHT - BUTTON_HEIGHT).abs() < f32::EPSILON);
    }

    #[test]
    fn toggle_pill_locked_to_32_by_16() {
        assert!((TOGGLE_WIDTH - 32.0).abs() < f32::EPSILON);
        assert!((TOGGLE_HEIGHT - 16.0).abs() < f32::EPSILON);
        assert!((TOGGLE_KNOB_DIAMETER - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn disabled_opacity_locked_to_40_pct() {
        assert!((DISABLED_OPACITY - 0.40).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_ring_locked_to_two_px_offset_one_px() {
        assert!((FOCUS_RING_WIDTH - 2.0).abs() < f32::EPSILON);
        assert!((FOCUS_RING_OFFSET - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scrollbar_width_locked_to_12() {
        assert!((SCROLLBAR_WIDTH - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn all_variants_construct() {
        let palette = crate::live_theme::palette();
        let _ = variant_button::<()>("p", ButtonVariant::Primary, None, palette);
        let _ = variant_button::<()>("s", ButtonVariant::Secondary, None, palette);
        let _ = variant_button::<()>("g", ButtonVariant::Ghost, None, palette);
    }

    #[test]
    fn skeleton_spinner_toggle_input_construct() {
        let palette = crate::live_theme::palette();
        let _ = skeleton::<()>(100.0, 20.0, palette);
        let _ = spinner::<()>(palette);
        let _ = toggle::<bool>(true, |v| v, palette);
        let _ = styled_text_input::<String>("p", "v", |s| s, palette);
    }

    #[test]
    fn checkbox_radio_scrollbar_style_construct() {
        let palette = crate::live_theme::palette();
        let _ = checkbox_style(palette);
        let _ = radio_style(palette);
        let _ = scrollbar_style(palette);
    }
}
