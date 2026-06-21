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
use cosmic::iced::{
    alignment, Background, Border, Color, Element, Length, Padding, Shadow, Vector,
};

use crate::cosmic_compat::prelude::*;
use mde_theme::animation::Transition;
use mde_theme::{FontSize, Palette, Radii, Shadow as MdeShadow, TypeRole};

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

// ─────────────────────────────────────────────────────────────────────────
// MOTION-FEEDBACK-1 — uniform hover-lift / focus-ring / press feedback.
//
// The shell's interactive controls (buttons, tabs, nav rows, toolbar icons)
// must share ONE hover/press/focus vocabulary instead of each call site
// inventing its own (header used `hover_tint`/`active_tint`; the sidebar used
// `raised`/`overlay` — visibly different feedback for the same gesture). This
// section is that single source: a pure, reduce-motion-aware mapping from
// iced's interaction `Status` to the Carbon-token visual deltas, reusing the
// MOTION-INFRA-2 `Transition::Lift`/`Press` math for the lift/depress
// magnitudes (§6 — glue, no re-implemented motion math).
//
// iced 0.13 (the libcosmic fork) has no opacity/transform widget, so the
// "lift" is expressed as a Carbon elevation shadow and the "press" as a
// collapsed shadow + the deeper `active_tint` — the same channels a real
// button can paint inside a stateless `button::Style` closure. Under
// reduce-motion the *movement* (lift shadow) is dropped but the *state*
// (tint, focus ring) still changes — the Q32 / FEEDBACK-1 contract: "keeps
// the visual state change without movement".
// ─────────────────────────────────────────────────────────────────────────

/// MOTION-FEEDBACK-1 — hover-lift rise at full press progress (px), the
/// magnitude fed to [`Transition::Lift`]. Carbon micro-interaction scale.
pub const HOVER_LIFT_PX: f32 = 2.0;

/// MOTION-FEEDBACK-1 — press depress depth fed to [`Transition::Press`]
/// (0.04 ⇒ 0.96 scale at full press). Used to derive the collapsed-elevation
/// feel on press.
pub const PRESS_DEPTH: f32 = 0.04;

/// MOTION-FEEDBACK-1 — the interaction state of a control, distilled from
/// iced's per-widget `button::Status`. The shared builders map their toolkit
/// status into this so the *feedback* decision is made in one pure place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// Idle — pointer away, not pressed.
    Rest,
    /// Pointer over the control (hover-lift + hover tint).
    Hover,
    /// Pointer down on the control (depress + active tint).
    Press,
    /// Non-interactive (dimmed; no hover/press feedback).
    Disabled,
}

impl Feedback {
    /// Map iced's `button::Status` into the shared feedback state.
    #[must_use]
    pub fn from_status(status: ButtonStatus) -> Self {
        match status {
            ButtonStatus::Hovered => Self::Hover,
            ButtonStatus::Pressed => Self::Press,
            ButtonStatus::Disabled => Self::Disabled,
            ButtonStatus::Active => Self::Rest,
        }
    }

    /// The hover-lift progress for this state, in `0.0..=1.0`: fully lifted on
    /// hover, settled (0.0) at rest / press / disabled. Drives
    /// [`Transition::Lift`] so the rise magnitude is the INFRA-2 math, not a
    /// local literal.
    #[must_use]
    pub fn lift_progress(self) -> f32 {
        match self {
            Self::Hover => 1.0,
            Self::Rest | Self::Press | Self::Disabled => 0.0,
        }
    }

    /// The press-depress progress for this state, in `0.0..=1.0`: fully
    /// depressed while pressed, flat otherwise. Drives [`Transition::Press`].
    #[must_use]
    pub fn press_progress(self) -> f32 {
        match self {
            Self::Press => 1.0,
            Self::Rest | Self::Hover | Self::Disabled => 0.0,
        }
    }
}

/// MOTION-FEEDBACK-1 — the resolved visual deltas for an interaction state:
/// the Carbon-token background overlay alpha, the elevation shadow, and the
/// press scale. Pure data — the builders fold these into their themed
/// `button::Style`. Keeping it a struct (not inline match arms scattered per
/// builder) is the whole point: every control resolves the same way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedbackStyle {
    /// Alpha of the Carbon accent wash laid over the control's base
    /// background. 0.0 = none (rest), [`Palette::hover_tint`]'s 0.08 on hover,
    /// [`Palette::active_tint`]'s 0.12 on press.
    pub tint_alpha: f32,
    /// Hover-lift rise in px (negative = up), from [`Transition::Lift`]. Zero
    /// under reduce-motion (state changes, movement does not).
    pub lift_px: f32,
    /// Press scale multiplier (1.0 = natural), from [`Transition::Press`].
    /// 1.0 under reduce-motion.
    pub press_scale: f32,
    /// Elevation shadow that renders the lift (an actual shadow stands in for
    /// the absent transform widget). [`MdeShadow::none`] at rest / press /
    /// reduce-motion.
    pub shadow: MdeShadow,
}

impl FeedbackStyle {
    /// MOTION-FEEDBACK-1 — resolve the visual deltas for `feedback`, honoring
    /// `reduce_motion` (movement collapses, state still changes).
    ///
    /// Magnitudes come from the MOTION-INFRA-2 transitions:
    /// [`Transition::Lift`] for the rise and [`Transition::Press`] for the
    /// depress scale, evaluated at the state's progress. The Carbon tint
    /// alphas mirror [`Palette::hover_tint`] (0.08) and
    /// [`Palette::active_tint`] (0.12) so the wash matches the palette tokens.
    #[must_use]
    pub fn resolve(feedback: Feedback, reduce_motion: bool) -> Self {
        let tint_alpha = match feedback {
            Feedback::Rest | Feedback::Disabled => 0.0,
            // Mirrors Palette::hover_tint() / active_tint() alphas (Carbon
            // accent wash). The builders apply the actual accent color.
            Feedback::Hover => 0.08,
            Feedback::Press => 0.12,
        };

        // Reuse the INFRA-2 transition math for the magnitudes. Under
        // reduce-motion the movement collapses to zero (no lift, no scale)
        // while the tint state above still changes.
        let (lift_px, press_scale, shadow) = if reduce_motion {
            (0.0, 1.0, MdeShadow::none())
        } else {
            let lift = Transition::Lift(HOVER_LIFT_PX)
                .params(feedback.lift_progress())
                .translate_y;
            let scale = Transition::Press(PRESS_DEPTH)
                .params(feedback.press_progress())
                .scale;
            // The lift is rendered as a Carbon elevation shadow (no transform
            // widget). Hover rises to SHADOW_1 (`lift`); rest/press sit flat.
            let shadow = if matches!(feedback, Feedback::Hover) {
                MdeShadow::lift()
            } else {
                MdeShadow::none()
            };
            (lift, scale, shadow)
        };

        Self {
            tint_alpha,
            lift_px,
            press_scale,
            shadow,
        }
    }

    /// Lay this state's Carbon accent wash over `base`, using `accent` as the
    /// wash hue. At `tint_alpha == 0.0` the base is returned unchanged.
    #[must_use]
    pub fn tinted_bg(self, base: Color, accent: Color) -> Color {
        if self.tint_alpha <= f32::EPSILON {
            return base;
        }
        // Alpha-composite the accent wash over the (opaque-treated) base so a
        // transparent base (ghost button / nav row) still shows the wash, and
        // a filled base (primary button) reads as a subtle accent shift.
        let a = self.tint_alpha;
        let over = |b: f32, w: f32| w * a + b * (1.0 - a);
        Color {
            r: over(base.r, accent.r),
            g: over(base.g, accent.g),
            b: over(base.b, accent.b),
            // Never reduce the base's own opacity; the wash only adds presence.
            a: base.a.max(a),
        }
    }

    /// The iced drop-shadow for this state's elevation.
    #[must_use]
    pub fn iced_shadow(self) -> Shadow {
        Shadow {
            color: self.shadow.color.into_cosmic_color(),
            offset: Vector::new(self.shadow.offset_x, self.shadow.offset_y),
            blur_radius: self.shadow.blur,
        }
    }
}

/// MOTION-FEEDBACK-1 — the animated 2 px Carbon focus ring border. Returns the
/// accent ring when `focused`, otherwise `base` (left untouched). Single source
/// so every shared control draws the identical focus affordance.
#[must_use]
pub fn focus_ring(focused: bool, base: Border, palette: Palette) -> Border {
    if focused {
        Border {
            color: palette.accent.into_cosmic_color(),
            width: FOCUS_RING_WIDTH,
            radius: base.radius,
        }
    } else {
        base
    }
}

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

    let reduce_motion = crate::live_theme::reduce_motion();
    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
        let base_bg = base_bg_for_variant(variant, accent, palette);
        let mut fg = text_color_for_variant(variant, palette);
        let mut border = border_for_variant(variant, accent, palette);

        // MOTION-FEEDBACK-1 — uniform hover-lift / press feedback from the
        // shared resolver (Carbon accent wash + INFRA-2 lift/press math),
        // instead of the old per-call-site luminance brighten.
        let feedback = Feedback::from_status(status);
        let fx = FeedbackStyle::resolve(feedback, reduce_motion);
        let mut bg = fx.tinted_bg(base_bg, accent);
        let mut shadow = fx.iced_shadow();

        if matches!(status, ButtonStatus::Disabled) {
            fg = with_alpha(fg, DISABLED_OPACITY);
            bg = with_alpha(bg, DISABLED_OPACITY * bg.a.max(0.1));
            border.color = with_alpha(border.color, DISABLED_OPACITY);
            shadow = Shadow::default();
        }
        button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color: fg,
            border,
            shadow,
            ..button::Style::default()
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
        .sty(style);
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
            let base = if value { bg_on } else { bg_off };
            // MOTION-FEEDBACK-1 — same hover/press wash as every other control
            // (no lift shadow on the pill — it's a fixed inline affordance).
            let fx = FeedbackStyle::resolve(
                Feedback::from_status(status),
                crate::live_theme::reduce_motion(),
            );
            let bg = fx.tinted_bg(base, accent);
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

    // ── MOTION-FEEDBACK-1 — pure feedback-state math ──────────────────────

    #[test]
    fn feedback_maps_every_button_status() {
        // The interaction → feedback-state mapping is the single decision
        // point; exhaustively pin each toolkit status to its state.
        assert_eq!(Feedback::from_status(ButtonStatus::Active), Feedback::Rest);
        assert_eq!(
            Feedback::from_status(ButtonStatus::Hovered),
            Feedback::Hover
        );
        assert_eq!(
            Feedback::from_status(ButtonStatus::Pressed),
            Feedback::Press
        );
        assert_eq!(
            Feedback::from_status(ButtonStatus::Disabled),
            Feedback::Disabled
        );
    }

    #[test]
    fn lift_and_press_progress_are_state_exclusive() {
        // Hover lifts but doesn't depress; Press depresses but doesn't lift;
        // Rest/Disabled do neither.
        assert_eq!(Feedback::Hover.lift_progress(), 1.0);
        assert_eq!(Feedback::Hover.press_progress(), 0.0);
        assert_eq!(Feedback::Press.press_progress(), 1.0);
        assert_eq!(Feedback::Press.lift_progress(), 0.0);
        for f in [Feedback::Rest, Feedback::Disabled] {
            assert_eq!(f.lift_progress(), 0.0);
            assert_eq!(f.press_progress(), 0.0);
        }
    }

    #[test]
    fn resolve_uses_infra2_lift_and_press_magnitudes() {
        // §6 — the magnitudes must come from MOTION-INFRA-2's Transition math,
        // not a local literal: hover rises by exactly Transition::Lift, press
        // scales by exactly Transition::Press.
        let hover = FeedbackStyle::resolve(Feedback::Hover, false);
        let expected_lift = Transition::Lift(HOVER_LIFT_PX).params(1.0).translate_y;
        assert!((hover.lift_px - expected_lift).abs() < 1e-6);
        assert!(
            (hover.lift_px - -HOVER_LIFT_PX).abs() < 1e-6,
            "hover rises up"
        );
        assert_eq!(hover.press_scale, 1.0, "hover does not depress");

        let press = FeedbackStyle::resolve(Feedback::Press, false);
        let expected_scale = Transition::Press(PRESS_DEPTH).params(1.0).scale;
        assert!((press.press_scale - expected_scale).abs() < 1e-6);
        assert!(press.press_scale < 1.0, "press depresses");
        assert_eq!(press.lift_px, 0.0, "press does not lift");
    }

    #[test]
    fn resolve_tint_alpha_mirrors_carbon_tokens() {
        // The wash alphas must mirror Palette::hover_tint (0.08) /
        // active_tint (0.12) so the feedback matches the palette tokens.
        let p = crate::live_theme::palette();
        let hover = FeedbackStyle::resolve(Feedback::Hover, false);
        let press = FeedbackStyle::resolve(Feedback::Press, false);
        assert!((hover.tint_alpha - p.hover_tint().a).abs() < 1e-6);
        assert!((press.tint_alpha - p.active_tint().a).abs() < 1e-6);
        // Rest + Disabled never wash.
        assert_eq!(
            FeedbackStyle::resolve(Feedback::Rest, false).tint_alpha,
            0.0
        );
        assert_eq!(
            FeedbackStyle::resolve(Feedback::Disabled, false).tint_alpha,
            0.0
        );
    }

    #[test]
    fn reduce_motion_keeps_state_drops_movement() {
        // FEEDBACK-1 / Q32 contract: reduce-motion keeps the visual STATE
        // change (the tint still flips on hover/press) but drops MOVEMENT
        // (no lift, no scale, no elevation shadow).
        let hover = FeedbackStyle::resolve(Feedback::Hover, true);
        assert!(
            hover.tint_alpha > 0.0,
            "state still changes under reduce-motion"
        );
        assert_eq!(hover.lift_px, 0.0, "no movement under reduce-motion");
        assert_eq!(hover.press_scale, 1.0, "no scale under reduce-motion");
        assert_eq!(
            hover.shadow,
            MdeShadow::none(),
            "no elevation under reduce-motion"
        );

        let press = FeedbackStyle::resolve(Feedback::Press, true);
        assert!(press.tint_alpha > 0.0);
        assert_eq!(press.press_scale, 1.0);
    }

    #[test]
    fn hover_elevates_rest_and_press_stay_flat() {
        // The lift is rendered as a Carbon elevation shadow (no transform
        // widget): only hover carries a non-zero drop shadow.
        assert_eq!(
            FeedbackStyle::resolve(Feedback::Hover, false).shadow,
            MdeShadow::lift()
        );
        for f in [Feedback::Rest, Feedback::Press, Feedback::Disabled] {
            assert_eq!(FeedbackStyle::resolve(f, false).shadow, MdeShadow::none());
        }
    }

    #[test]
    fn tinted_bg_is_identity_at_rest_and_washes_otherwise() {
        let base = Color::from_rgb(0.1, 0.1, 0.1); // carbon-ok: test fixture
        let accent = Color::from_rgb(0.0, 0.4, 1.0); // carbon-ok: test fixture
        let rest = FeedbackStyle::resolve(Feedback::Rest, false);
        // Rest: untouched.
        assert_eq!(rest.tinted_bg(base, accent), base);
        // Hover: shifted toward the accent (blue channel rises).
        let hover = FeedbackStyle::resolve(Feedback::Hover, false);
        let washed = hover.tinted_bg(base, accent);
        assert!(washed.b > base.b, "hover wash pulls toward accent");
        // Press wash is stronger than hover (12% vs 8%).
        let press = FeedbackStyle::resolve(Feedback::Press, false);
        let pressed = press.tinted_bg(base, accent);
        assert!(pressed.b > washed.b, "press wash is deeper than hover");
    }

    #[test]
    fn tinted_bg_reveals_wash_over_transparent_base() {
        // Ghost buttons / nav rows have a TRANSPARENT base — the wash must
        // still become visible (alpha rises to the wash alpha).
        let accent = Color::from_rgb(0.0, 0.4, 1.0); // carbon-ok: test fixture
        let hover = FeedbackStyle::resolve(Feedback::Hover, false);
        let washed = hover.tinted_bg(Color::TRANSPARENT, accent);
        assert!((washed.a - hover.tint_alpha).abs() < 1e-6);
    }

    #[test]
    fn focus_ring_applies_carbon_accent_only_when_focused() {
        let palette = crate::live_theme::palette();
        let base = Border::default();
        let unfocused = focus_ring(false, base, palette);
        assert_eq!(unfocused.width, base.width, "unfocused leaves border as-is");
        let focused = focus_ring(true, base, palette);
        assert!((focused.width - FOCUS_RING_WIDTH).abs() < f32::EPSILON);
        let accent = palette.accent.into_cosmic_color();
        assert_eq!(focused.color, accent, "focus ring is the Carbon accent");
        // The ring preserves the control's corner radius.
        let rounded = Border {
            radius: 8.0.into(),
            ..Border::default()
        };
        assert_eq!(focus_ring(true, rounded, palette).radius, rounded.radius);
    }

    #[test]
    fn iced_shadow_round_trips_the_token() {
        let hover = FeedbackStyle::resolve(Feedback::Hover, false);
        let s = hover.iced_shadow();
        assert!((s.blur_radius - MdeShadow::lift().blur).abs() < 1e-6);
        assert!((s.offset.y - MdeShadow::lift().offset_y).abs() < 1e-6);
    }

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
