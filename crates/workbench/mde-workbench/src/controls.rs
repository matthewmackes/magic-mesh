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

use cosmic::iced::widget::button::Status as ButtonStatus;
use cosmic::iced::widget::checkbox::{Status as CheckboxStatus, Style as CheckboxStyle};
use cosmic::iced::widget::radio::{Status as RadioStatus, Style as RadioStyle};
use cosmic::iced::widget::scrollable::{
    Rail, Scroller, Status as ScrollStatus, Style as ScrollStyle,
};
use cosmic::iced::widget::{button, container, row, text, text_input, Space};
use cosmic::iced::{alignment, Background, Border, Color, Element, Length, Padding, Shadow};

use std::time::{Duration, Instant};

use crate::cosmic_compat::prelude::*;
use mde_theme::animation::lerp_f32;
use mde_theme::feedback::ControlFeedback;
use mde_theme::{
    FontSize, Palette, Radii, TypeRole, CARD_HOVER_OVERLAY_ALPHA, CARD_SELECTED_OVERLAY_ALPHA,
    CARD_SHADOW_HOVER_ALPHA, CARD_SHADOW_HOVER_BLUR, CARD_SHADOW_HOVER_OFFSET_Y,
};

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
) -> Element<'a, Message, cosmic::Theme> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_cosmic_color();
    let text_role = TypeRole::Body;
    let label_text = text(label.into())
        .size(text_role.size_in(sizes))
        .colr(text_color_for_variant(variant, palette))
        .align_y(alignment::Vertical::Center);

    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
        // Only the interactive arms consult the live a11y flags — the Hovered arm
        // for the decorative lift-shadow (§Q32 + MOTION-A11Y-2), the Pressed arm
        // for the MOTION-FEEDBACK-1 focus ring — so read them lazily on those arms
        // alone. `variant_button` is a shared helper on 100+ call sites and the
        // accessor loads the prefs file; reading it for every button every frame
        // would be a per-render disk-I/O storm. At most one button is in each of
        // those states per frame, matching the established once-per-view cost.
        let (reduce_motion, decorative) = match status {
            ButtonStatus::Hovered | ButtonStatus::Pressed => (
                crate::live_theme::reduce_motion(),
                crate::live_theme::decorative_motion(),
            ),
            ButtonStatus::Active | ButtonStatus::Disabled => (false, true),
        };
        variant_button_style(
            variant,
            accent,
            palette,
            reduce_motion,
            decorative,
            Instant::now(),
            status,
        )
    };

    let mut btn = button(label_text)
        .padding(Padding {
            top: 0.0,
            right: BUTTON_HORIZONTAL_PADDING,
            bottom: 0.0,
            left: BUTTON_HORIZONTAL_PADDING,
        })
        .height(Length::Fixed(BUTTON_HEIGHT))
        .sty(style);
    if let Some(msg) = on_press {
        btn = btn.on_press(msg);
    }
    btn.into()
}

/// MOTION-FEEDBACK-1 — the pure status→[`button::Style`] mapping behind
/// [`variant_button`]'s render closure. Keyed off the widget's
/// [`ButtonStatus`], it applies the shared hover/press feedback over the locked
/// Carbon chrome:
///
///   * **Hovered** — a subtle accent tint ([`hover_tint`], always applied) plus
///     an upward hover-lift drawn as a drop shadow ([`hover_lift_shadow`]).
///     The lift is **decorative** motion: dropped under `reduce_motion` (§Q32)
///     and when the user disables non-essential motion (`!decorative`,
///     MOTION-A11Y-2); the always-on accent tint carries the hover *state*
///     either way. Height, padding, and border weight are untouched — the size
///     lock and the structural border are preserved.
///   * **Pressed** — a press-down darken ([`press_tint`]) with no input delay,
///     plus the MOTION-FEEDBACK-1 animated accent **focus ring** drawn through the
///     shared [`ControlFeedback::focus_ring`] vocabulary. iced's
///     [`ButtonStatus`] carries no keyboard-focus signal (it can't be surfaced to
///     a style closure), so — exactly as the Overview's `feedback_button` does —
///     the engaged/pressed state is the focus-like cue the ring marks. The ring
///     grows in under motion and snaps to full width/opacity under reduce-motion
///     (the helper's a11y contract); it blends the border toward `accent` and
///     thickens it, never touching height/padding.
///   * **Disabled** — fades fg/bg/border by [`DISABLED_OPACITY`] (the const).
///
/// Extracted so the status→style mapping is unit-testable (the closure itself
/// can't be reached from a test).
fn variant_button_style(
    variant: ButtonVariant,
    accent: Color,
    palette: Palette,
    reduce_motion: bool,
    decorative: bool,
    now: Instant,
    status: ButtonStatus,
) -> button::Style {
    let base_bg = base_bg_for_variant(variant, accent, palette);
    let mut bg = base_bg;
    let mut fg = text_color_for_variant(variant, palette);
    let mut border = border_for_variant(variant, accent, palette);
    // Resting: no lift, no press-tint. Hovered/Pressed override below.
    let mut shadow = Shadow::default();
    match status {
        ButtonStatus::Hovered => {
            bg = hover_tint(base_bg, accent);
            // The lift is decorative movement — suppressed under reduce-motion OR
            // when non-essential motion is disabled. The accent tint above is the
            // reduce-motion-safe, always-on state cue.
            shadow = hover_lift_shadow(reduce_motion || !decorative);
        }
        ButtonStatus::Pressed => {
            bg = press_tint(base_bg);
            // MOTION-FEEDBACK-1 — draw the shared animated focus ring on the
            // engaged control. A settled-in-the-past `since` so a render-time
            // consumer (no per-button tick) reads the fully-grown ring at `now`;
            // under reduce-motion the helper snaps it to full immediately.
            let settled = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
            let ring = ControlFeedback::new()
                .focused(true, settled)
                .focus_ring(now, reduce_motion);
            if ring.is_visible() {
                let a = ring.alpha.clamp(0.0, 1.0);
                border.color = blend(border.color, accent, a);
                border.width += ring.width;
            }
        }
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
        shadow,
        ..button::Style::default()
    }
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
        ButtonVariant::Secondary => palette.accent.into_cosmic_color(),
        ButtonVariant::Ghost => palette.text.into_cosmic_color(),
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
    _palette: Palette,
) -> Element<'a, Message, cosmic::Theme> {
    // CUT-1 fork-drift note: the libcosmic `cosmic::Theme` text_input Catalog
    // (cosmic/src/theme/style/iced.rs — `enum TextInput { Default, Search }`,
    // marked `TODO: Text Input`) has NO per-instance closure variant, so the
    // crates.io-iced `.style(|theme, status| ...)` Carbon-token closure cannot
    // be threaded here. Select the built-in `Default` class until the fork
    // grows a custom text_input variant (then re-thread the palette closure).
    text_input(placeholder, value)
        .on_input(on_input)
        .padding(Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        })
        .size(13)
        .class(cosmic::theme::iced::TextInput::Default)
        .into()
}

/// CR-9 — toggle pill. 32×16 px, 12 px knob, palette.border off bg,
/// accent on bg. Slide animation (140 ms ease-out per spec) deferred
/// to UX-9.a subscription wiring — stateless snap for now.
pub fn toggle<'a, Message: Clone + 'a>(
    value: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
    palette: Palette,
) -> Element<'a, Message, cosmic::Theme> {
    let radii = Radii::defaults();
    let accent = palette.accent.into_cosmic_color();
    let bg_off = palette.border.into_cosmic_color();
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
        .sty(move |_theme, status| {
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
                ..button::Style::default()
            }
        })
        .into()
}

/// CR-9 — checkbox style closure for `iced::widget::checkbox::style()`.
/// 16 px sharp square (set via `.size(16)` on the widget), accent fill
/// when checked, white checkmark icon.
pub fn checkbox_style(
    palette: Palette,
) -> impl Fn(&cosmic::Theme, CheckboxStatus) -> CheckboxStyle {
    let accent = palette.accent.into_cosmic_color();
    let divider = palette.border.into_cosmic_color();
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
pub fn radio_style(palette: Palette) -> impl Fn(&cosmic::Theme, RadioStatus) -> RadioStyle {
    let accent = palette.accent.into_cosmic_color();
    let divider = palette.border.into_cosmic_color();
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
pub fn scrollbar_style(palette: Palette) -> impl Fn(&cosmic::Theme, ScrollStatus) -> ScrollStyle {
    let track_bg = palette.surface.into_cosmic_color();
    let thumb_default = palette.border.into_cosmic_color();
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
            auto_scroll: cosmic::iced::widget::scrollable::AutoScroll {
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
) -> Element<'a, Message, cosmic::Theme> {
    let radii = Radii::defaults();
    let bg = with_alpha(palette.raised.into_cosmic_color(), 0.6);
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
pub fn spinner<'a, Message: 'a>(palette: Palette) -> Element<'a, Message, cosmic::Theme> {
    let radii = Radii::defaults();
    let accent = palette.accent.into_cosmic_color();
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

/// MOTION-FEEDBACK-1 — opacity above which a button's resting fill is treated as
/// "already filled" (lighten on hover) vs. transparent (wash toward accent).
/// The button variants rest at exactly 1.0 (Primary) or 0.0 (Secondary/Ghost),
/// so a mid-point cutoff classifies them unambiguously and keeps the rule sane
/// for any future translucent fill.
const OPAQUE_FILL_CUTOFF: f32 = 0.5;

/// MOTION-FEEDBACK-1 — the hover tint (the state cue, kept even under
/// reduce-motion), applied without touching height, padding, or border weight.
/// Single-sources its strength on the Carbon hover-overlay token
/// ([`CARD_HOVER_OVERLAY_ALPHA`]) — the same 8% the Object Card uses — so the
/// button's hover reads at the platform-standard strength, not a re-derived
/// literal.
///
/// The tint *direction* depends on the variant's resting fill so every variant
/// reads a hover change:
///   * an **accent-filled** button (Primary) would show no shift blending toward
///     its own accent, so it **lightens toward white** — a hover highlight.
///   * a **transparent** button (Secondary/Ghost) gains an **accent wash**
///     (blends toward accent, alpha lifting off zero) — a visible accent fill
///     appearing under the pointer.
fn hover_tint(base: Color, accent: Color) -> Color {
    if base.a > OPAQUE_FILL_CUTOFF {
        blend(base, Color::WHITE, CARD_HOVER_OVERLAY_ALPHA)
    } else {
        blend(base, accent, CARD_HOVER_OVERLAY_ALPHA)
    }
}

/// MOTION-FEEDBACK-1 — the press darken (the press-down state cue). Darkens the
/// resting background toward black at the Carbon selected/engaged-overlay
/// strength ([`CARD_SELECTED_OVERLAY_ALPHA`], 15% — deeper than the 8% hover so
/// the press reads as a firmer depress). Applied on the `Pressed` status with
/// no input delay (it is keyed off the status, not a warm-up tween).
fn press_tint(base: Color) -> Color {
    blend(base, Color::BLACK, CARD_SELECTED_OVERLAY_ALPHA)
}

/// MOTION-FEEDBACK-1 — the hover-lift, expressed at render time as a drop shadow
/// (the widget itself can't translate from a style closure, so the shadow casts
/// the "raised above the surface" cue). Single-sources its offset/blur/alpha on
/// the Carbon hover-shadow tokens — the same raised shadow the Object Card lifts
/// to on hover — rather than re-deriving shadow metrics.
///
/// This is *movement*: under reduce-motion it collapses to no shadow (§Q32 — the
/// accent tint still carries the hover state), so motion drops while the state
/// stays.
fn hover_lift_shadow(reduce_motion: bool) -> Shadow {
    if reduce_motion {
        return Shadow::default();
    }
    Shadow {
        color: with_alpha(Color::BLACK, CARD_SHADOW_HOVER_ALPHA),
        offset: cosmic::iced::Vector::new(0.0, CARD_SHADOW_HOVER_OFFSET_Y),
        blur_radius: CARD_SHADOW_HOVER_BLUR,
    }
}

/// Blend `from` toward `to` by `t` (0.0 = `from`, 1.0 = `to`), per channel
/// including alpha. Reuses the shared [`lerp_f32`] so the interpolation math
/// lives in one place. Interpolating alpha means a transparent fill (the
/// secondary/ghost variants) gains a faint accent/press wash on
/// hover/press — a consistent cue for every variant — while an opaque fill
/// (primary) stays opaque.
fn blend(from: Color, to: Color, t: f32) -> Color {
    Color {
        r: lerp_f32(from.r, to.r, t),
        g: lerp_f32(from.g, to.g, t),
        b: lerp_f32(from.b, to.b, t),
        a: lerp_f32(from.a, to.a, t),
    }
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

    // MOTION-FEEDBACK-1 — status→style mapping for the shared variant_button.

    fn style_for(variant: ButtonVariant, reduce_motion: bool, status: ButtonStatus) -> button::Style {
        // Default to decorative-on (full polish) so the existing hover/press
        // assertions read the standard chrome; the decorative-off path has its own
        // test below.
        style_for_full(variant, reduce_motion, true, status)
    }

    fn style_for_full(
        variant: ButtonVariant,
        reduce_motion: bool,
        decorative: bool,
        status: ButtonStatus,
    ) -> button::Style {
        let palette = crate::live_theme::palette();
        let accent = palette.accent.into_cosmic_color();
        variant_button_style(
            variant,
            accent,
            palette,
            reduce_motion,
            decorative,
            Instant::now(),
            status,
        )
    }

    fn bg_color(style: &button::Style) -> Color {
        match style.background {
            Some(Background::Color(c)) => c,
            other => panic!("expected a solid background color, got {other:?}"),
        }
    }

    #[test]
    fn hover_tints_toward_accent_and_lifts_with_a_shadow() {
        // Hovered: bg shifts toward accent (the always-on color cue) AND a lift
        // shadow appears (the movement) when motion is on.
        let active = style_for(ButtonVariant::Primary, false, ButtonStatus::Active);
        let hovered = style_for(ButtonVariant::Primary, false, ButtonStatus::Hovered);
        assert_ne!(
            bg_color(&active),
            bg_color(&hovered),
            "hover must change the background (accent tint)"
        );
        // Movement: a raised lift shadow at the Carbon hover-shadow strength.
        assert!(active.shadow.offset.y.abs() < f32::EPSILON, "resting button has no lift");
        assert!(
            (hovered.shadow.offset.y - CARD_SHADOW_HOVER_OFFSET_Y).abs() < f32::EPSILON,
            "hover lift uses the Carbon hover-shadow offset"
        );
        assert!(
            (hovered.shadow.color.a - CARD_SHADOW_HOVER_ALPHA).abs() < f32::EPSILON,
            "lift shadow is visible at the Carbon hover-shadow alpha"
        );
    }

    #[test]
    fn hover_keeps_the_size_lock_and_border_weight() {
        // The hover feedback must not touch the structural chrome: border width
        // is unchanged from the resting (Active) border, and the helper never
        // sets a height/padding (those live on the widget, not the style).
        for variant in [ButtonVariant::Primary, ButtonVariant::Secondary, ButtonVariant::Ghost] {
            let active = style_for(variant, false, ButtonStatus::Active);
            let hovered = style_for(variant, false, ButtonStatus::Hovered);
            assert!(
                (active.border.width - hovered.border.width).abs() < f32::EPSILON,
                "{variant:?}: hover must preserve the border weight (no focus-ring repurposing)"
            );
        }
    }

    #[test]
    fn reduce_motion_keeps_the_hover_tint_but_drops_the_lift() {
        // §Q32: under reduce-motion the hover *state* (the accent tint) is kept,
        // but the *movement* (the lift shadow) is dropped.
        let full = style_for(ButtonVariant::Primary, false, ButtonStatus::Hovered);
        let reduced = style_for(ButtonVariant::Primary, true, ButtonStatus::Hovered);
        // Tint kept: the hovered background is identical with/without motion.
        assert_eq!(
            bg_color(&full),
            bg_color(&reduced),
            "the hover accent tint stays under reduce-motion"
        );
        // Movement dropped: no lift shadow.
        assert!(
            reduced.shadow.offset.y.abs() < f32::EPSILON && reduced.shadow.color.a < f32::EPSILON,
            "no lift shadow under reduce-motion"
        );
    }

    #[test]
    fn press_darkens_relative_to_hover_with_no_lift() {
        // Pressed: a press-down darken, deeper than hover, and no lift shadow
        // (the depress is a sink, not a rise).
        let hovered = style_for(ButtonVariant::Primary, false, ButtonStatus::Hovered);
        let pressed = style_for(ButtonVariant::Primary, false, ButtonStatus::Pressed);
        let h = bg_color(&hovered);
        let p = bg_color(&pressed);
        // Darken: each RGB channel of pressed is <= hovered (toward black).
        assert!(
            p.r <= h.r && p.g <= h.g && p.b <= h.b && (p.r < h.r || p.g < h.g || p.b < h.b),
            "press darkens the background relative to hover"
        );
        assert!(pressed.shadow.offset.y.abs() < f32::EPSILON, "no lift on press");
    }

    #[test]
    fn disabled_uses_the_disabled_opacity_const() {
        // Disabled fades fg/bg/border by DISABLED_OPACITY (the const, not 0.40).
        let primary = style_for(ButtonVariant::Primary, false, ButtonStatus::Disabled);
        // Primary's white text faded to DISABLED_OPACITY alpha.
        assert!(
            (primary.text_color.a - DISABLED_OPACITY).abs() < f32::EPSILON,
            "disabled text alpha == DISABLED_OPACITY"
        );
        // Secondary's accent border faded to DISABLED_OPACITY alpha.
        let secondary = style_for(ButtonVariant::Secondary, false, ButtonStatus::Disabled);
        assert!(
            (secondary.border.color.a - DISABLED_OPACITY).abs() < f32::EPSILON,
            "disabled border alpha == DISABLED_OPACITY"
        );
    }

    #[test]
    fn pressed_draws_the_animated_focus_ring_via_the_shared_helper() {
        // MOTION-FEEDBACK-1 — the engaged (pressed) control gains the accent focus
        // ring drawn through ControlFeedback::focus_ring: the border blends toward
        // accent and thickens past its resting weight. Active (resting) has no ring.
        for variant in [ButtonVariant::Primary, ButtonVariant::Secondary, ButtonVariant::Ghost] {
            let active = style_for(variant, false, ButtonStatus::Active);
            let pressed = style_for(variant, false, ButtonStatus::Pressed);
            assert!(
                pressed.border.width > active.border.width,
                "{variant:?}: the focus ring must thicken the border past rest \
                 ({} !> {})",
                pressed.border.width,
                active.border.width
            );
            // The ring colour is the accent (engaged control is accent-outlined).
            let accent = crate::live_theme::palette().accent.into_cosmic_color();
            assert!(
                (pressed.border.color.r - accent.r).abs() < 1e-3
                    && (pressed.border.color.g - accent.g).abs() < 1e-3
                    && (pressed.border.color.b - accent.b).abs() < 1e-3,
                "{variant:?}: the focus ring is the accent colour"
            );
        }
    }

    #[test]
    fn focus_ring_is_present_under_reduce_motion_just_not_animated() {
        // MOTION-FEEDBACK-1 / a11y: reduce-motion keeps the ring (the focus STATE
        // is a real cue, never dropped) — it just snaps to full instead of growing
        // in. So a pressed control is still accent-ringed with reduce-motion on.
        let pressed_full = style_for(ButtonVariant::Ghost, false, ButtonStatus::Pressed);
        let pressed_reduced = style_for(ButtonVariant::Ghost, true, ButtonStatus::Pressed);
        let active = style_for(ButtonVariant::Ghost, false, ButtonStatus::Active);
        assert!(pressed_reduced.border.width > active.border.width, "ring kept under reduce-motion");
        // At the settled render frame both reach the full ring width.
        assert!((pressed_reduced.border.width - pressed_full.border.width).abs() < 1e-3);
    }

    #[test]
    fn decorative_off_drops_the_hover_lift_but_keeps_the_tint() {
        // MOTION-A11Y-2 — disabling non-essential motion removes the hover-lift
        // shadow (decorative movement) while the accent hover tint (the state cue)
        // stays. Motion is globally ON (reduce_motion=false), only decorative off.
        let deco_on = style_for_full(ButtonVariant::Primary, false, true, ButtonStatus::Hovered);
        let deco_off = style_for_full(ButtonVariant::Primary, false, false, ButtonStatus::Hovered);
        // Lift dropped: no shadow under decorative-off.
        assert!(deco_on.shadow.offset.y.abs() > f32::EPSILON, "lift present with decorative on");
        assert!(
            deco_off.shadow.offset.y.abs() < f32::EPSILON && deco_off.shadow.color.a < f32::EPSILON,
            "no lift shadow when non-essential motion is disabled"
        );
        // Tint kept: the hovered background is identical either way.
        assert_eq!(bg_color(&deco_on), bg_color(&deco_off), "the hover tint stays");
    }

    #[test]
    fn transparent_variants_gain_a_visible_hover_wash() {
        // Secondary/Ghost rest on a transparent fill; the accent tint must still
        // read on hover (alpha lifts off zero) so every variant gets feedback.
        for variant in [ButtonVariant::Secondary, ButtonVariant::Ghost] {
            let active = style_for(variant, false, ButtonStatus::Active);
            let hovered = style_for(variant, false, ButtonStatus::Hovered);
            assert!(bg_color(&active).a < f32::EPSILON, "{variant:?} rests transparent");
            assert!(
                bg_color(&hovered).a > f32::EPSILON,
                "{variant:?} gains a visible hover wash"
            );
        }
    }
}
