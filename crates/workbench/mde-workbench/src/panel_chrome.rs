//! UX-6 — shared panel chrome.
//!
//! Every Iced panel pulls its outer padding, section header
//! rhythm, data-row grid, status badge shape, card surface, and
//! empty-state from this module. Before UX-6 each panel
//! rolled its own — the result was 32 panels with 32 slightly
//! different visual rhythms.
//!
//! Token rules (UX-6 spec):
//!   * outer panel padding = `SPACE_24` (≈ `Space::lg2` 24 px)
//!   * section header bottom gap = `SPACE_16` (≈ `Space::md2` 17)
//!   * row height = 44 px minimum (component dimension)
//!   * data label/value = 2-column 40/60 split
//!   * status badge = `Radii::full` (pill)
//!   * card = surface + `Shadow::lift()` + `Radii::md` corners
//!   * empty-state = the `EmptyState` data form + `empty_state()`
//!     renderer in this module
//!
//! Component dimensions (44 px row, 32 px icon slot) are NOT
//! density-scaled per UX-24 sub-lock.

use iced::widget::button::Status as ButtonStatus;
use iced::widget::{button, column, container, row, text, Column, Space};
use iced::{
    alignment, Background, Border, Color, Element, Font, Length, Padding, Shadow as IcedShadow,
};

use mde_theme::{
    components::empty_state::{BODY_CTA_GAP, EMPTY_ICON_SIZE, HEADING_BODY_GAP, VERTICAL_PADDING},
    mde_icon,
    motion::dialog as dialog_tokens,
    Density, EmptyState, FontSize, IconSize, Palette, Radii, Shadow as MdeShadow,
    Space as MdeSpace, TypeRole,
};

// CR-3.b — `object_card` extracted to `mde-iced-components` so
// peer crates (mde-files, mde-popover, mde-music, etc.) can render
// Object Cards without taking a heavyweight dep on mde-workbench.
// Re-exported here so existing workbench call sites stay
// unchanged.
pub use mde_iced_components::{object_card, overlay_color_on, overlay_white_on, with_alpha};

/// UX-6 — minimum data-row height. Component dimension, not
/// density-scaled.
pub const DATA_ROW_MIN_HEIGHT: f32 = 44.0;

/// UX-6 — outer panel padding (~SPACE_24 token).
pub fn outer_padding(density: Density) -> Padding {
    let space = MdeSpace::for_density(density);
    Padding {
        top: f32::from(space.lg2),
        right: f32::from(space.lg2),
        bottom: f32::from(space.lg2),
        left: f32::from(space.lg2),
    }
}

/// UX-6 — wrap a panel body in the standard outer container.
/// Applies `outer_padding(density)` and fills the available
/// area.
pub fn panel_container<'a, Message: 'a>(
    body: Element<'a, Message>,
    density: Density,
) -> Element<'a, Message> {
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(outer_padding(density))
        .into()
}

/// UX-6 — section header. `TypeRole::Section` text + SPACE_16
/// bottom gap absorbed by callers via column spacing.
pub fn section_header<'a, Message: 'a>(
    title: impl Into<String>,
    palette: Palette,
) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    text(title.into())
        .size(TypeRole::Section.size_in(sizes))
        .color(palette.text.into_iced_color())
        .into()
}

/// UX-6 — section block: section header + the caller's content,
/// separated by SPACE_16. Standard wrapper to avoid every panel
/// hand-rolling the same `column![header, body].spacing(16)`.
pub fn section_block<'a, Message: 'a>(
    title: impl Into<String>,
    body: Element<'a, Message>,
    palette: Palette,
    density: Density,
) -> Element<'a, Message> {
    let space = MdeSpace::for_density(density);
    column![section_header(title, palette), body]
        .spacing(f32::from(space.md2))
        .into()
}

/// UX-6 — data row: 2-column label/value grid, label 40%, value
/// 60%, 44 px minimum height. The label uses muted text; the
/// value uses primary text. Both render as plain `text()` —
/// the caller is responsible for wrapping the value side in a
/// link / badge / button if the row is interactive.
pub fn data_row<'a, Message: 'a + Clone>(
    label: impl Into<String>,
    value: impl Into<String>,
    palette: Palette,
) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    let label_text = text(label.into())
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text_muted.into_iced_color())
        .align_y(alignment::Vertical::Center)
        .width(Length::FillPortion(40));
    let value_text = text(value.into())
        .size(TypeRole::Body.size_in(sizes))
        .color(palette.text.into_iced_color())
        .align_y(alignment::Vertical::Center)
        .width(Length::FillPortion(60));
    row![label_text, value_text]
        .align_y(alignment::Vertical::Center)
        .height(Length::Fixed(DATA_ROW_MIN_HEIGHT))
        .spacing(8)
        .into()
}

/// Severity of a status badge — controls fill colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeSeverity {
    /// Neutral / muted — default for "unknown" / "not yet run".
    Neutral,
    /// Success / OK — green fill.
    Success,
    /// Warning — amber fill.
    Warning,
    /// Danger / failure — red fill.
    Danger,
    /// Info — accent (indigo) fill.
    Info,
}

/// UX-6 — pill-shaped status badge. RADIUS_FULL corners, ~6 px
/// horizontal padding, severity-tinted background.
pub fn status_badge<'a, Message: 'a>(
    label: impl Into<String>,
    severity: BadgeSeverity,
    palette: Palette,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let sizes = FontSize::defaults();
    let (bg, fg) = match severity {
        BadgeSeverity::Neutral => (
            palette.raised.into_iced_color(),
            palette.text.into_iced_color(),
        ),
        BadgeSeverity::Success => (
            Color::from_rgba(0.247, 0.725, 0.314, 0.20),
            Color::from_rgb(0.247, 0.725, 0.314),
        ),
        BadgeSeverity::Warning => (
            Color::from_rgba(0.961, 0.620, 0.043, 0.20),
            Color::from_rgb(0.961, 0.620, 0.043),
        ),
        BadgeSeverity::Danger => (
            Color::from_rgba(0.898, 0.325, 0.294, 0.20),
            Color::from_rgb(0.898, 0.325, 0.294),
        ),
        BadgeSeverity::Info => (
            palette.hover_tint().into_iced_color(),
            palette.accent.into_iced_color(),
        ),
    };

    container(
        text(label.into())
            .size(TypeRole::Caption.size_in(sizes))
            .color(fg)
            .align_y(alignment::Vertical::Center),
    )
    .padding(Padding {
        top: 4.0,
        right: 10.0,
        bottom: 4.0,
        left: 10.0,
    })
    .style(move |_theme| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: f32::from(radii.full).into(),
        },
        shadow: IcedShadow::default(),
        text_color: Some(fg),
    })
    .into()
}

/// UX-6 — card surface. Wraps any content in a raised surface
/// with `Shadow::lift()` elevation, `Radii::md` corners,
/// `space.lg` inner padding. Use for fleet peer cards, snapshot
/// cards, and any panel surface that needs to read as a discrete
/// container above the panel background.
pub fn card<'a, Message: 'a>(
    body: Element<'a, Message>,
    palette: Palette,
    density: Density,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let space = MdeSpace::for_density(density);
    container(body)
        .width(Length::Fill)
        .padding(Padding {
            top: f32::from(space.lg),
            right: f32::from(space.lg),
            bottom: f32::from(space.lg),
            left: f32::from(space.lg),
        })
        .style(move |_theme| container::Style {
            snap: false,
            background: Some(Background::Color(palette.surface.into_iced_color())),
            border: Border {
                color: palette.border.into_iced_color(),
                width: 1.0,
                radius: f32::from(radii.md).into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::lift()),
            text_color: Some(palette.text.into_iced_color()),
        })
        .into()
}

/// UX-6 — empty-state renderer. Take ownership of `EmptyState`
/// so callers can construct it inline (`EmptyState::info(…)`)
/// and pass it straight in — the strings get moved into the
/// iced widgets, no clones required at the call site. `on_cta`
/// is invoked when the CTA button (if any) is pressed.
pub fn empty_state<'a, Message: Clone + 'a>(
    state: EmptyState,
    palette: Palette,
    on_cta: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    let body_color = state
        .body_color_override
        .unwrap_or(palette.text_muted)
        .into_iced_color();

    // UX-8 — render the hero icon when set; otherwise reserve
    // the slot as empty space so the body block centers
    // consistently across panels that opt out of the icon.
    //
    // v4.0.1 BUG-13.c: prefer the baked Material Symbols SVG via
    // `Icon::svg_bytes()` (every variant now resolves to Some).
    // The Unicode fallback_glyph path stays as a safety net for
    // any future variant that ships an unbaked Icon.
    let icon_slot: Element<'a, Message> = if let Some(icon) = state.icon {
        let resolved = mde_icon(icon, IconSize::EmptyState);
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use iced::widget::svg as widget_svg;
            let muted = palette.text_muted.into_iced_color();
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(resolved.size_px()))
                .height(Length::Fixed(resolved.size_px()))
                .style(
                    move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                        color: Some(muted),
                    },
                )
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(resolved.size_px())
                .color(palette.text_muted.into_iced_color())
                .align_x(alignment::Horizontal::Center)
                .into()
        }
    } else {
        Space::new().height(Length::Fixed(EMPTY_ICON_SIZE)).into()
    };
    let heading = text(state.heading)
        .size(TypeRole::Heading.size_in(sizes))
        .color(palette.text.into_iced_color())
        .align_x(alignment::Horizontal::Center);
    let body = text(state.body)
        .size(TypeRole::Body.size_in(sizes))
        .color(body_color)
        .align_x(alignment::Horizontal::Center);

    let mut col: Column<'a, Message> = column![icon_slot, heading, body]
        .spacing(HEADING_BODY_GAP as f32)
        .align_x(alignment::Horizontal::Center);

    if let Some(label) = state.cta_label {
        let accent_color = palette.accent.into_iced_color();
        let radii = Radii::defaults();
        let cta_button: Element<'a, Message> = button(
            text(label)
                .size(TypeRole::Body.size_in(sizes))
                .color(Color::WHITE),
        )
        .padding(Padding {
            top: 8.0,
            right: 20.0,
            bottom: 8.0,
            left: 20.0,
        })
        .on_press(on_cta())
        .style(move |_theme, status: ButtonStatus| {
            let bg = match status {
                ButtonStatus::Hovered => brighten(accent_color, 1.10),
                ButtonStatus::Pressed => brighten(accent_color, 0.90),
                _ => accent_color,
            };
            button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: f32::from(radii.md).into(),
                },
                shadow: IcedShadow::default(),
            }
        })
        .into();

        col = col.push(Space::new().height(Length::Fixed(BODY_CTA_GAP)));
        col = col.push(cta_button);
    }

    container(col)
        .width(Length::Fill)
        .padding(Padding {
            top: VERTICAL_PADDING,
            right: 24.0,
            bottom: VERTICAL_PADDING,
            left: 24.0,
        })
        .align_x(alignment::Horizontal::Center)
        .into()
}

fn mde_shadow_to_iced(s: MdeShadow) -> IcedShadow {
    IcedShadow {
        color: s.color.into_iced_color(),
        offset: iced::Vector::new(s.offset_x, s.offset_y),
        blur_radius: s.blur,
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

/// CR-10 — dialog chrome. Wraps an arbitrary body in the
/// Classic ChromeOS modal shell: 16 px H padding, 480 px
/// max-width, 4 px corners (Radii::sm per chromeos-classic-spec
/// §Dialog 2026-05-24), `Shadow::modal()` drop shadow,
/// palette.raised background, 1 px border.
///
/// Pair with a backdrop overlay in the app's top-level view —
/// the caller composes `stack![backdrop, dialog]` or uses
/// `iced::widget::stack`. This function returns just the dialog
/// surface so consumers can position it freely.
pub fn dialog<'a, Message: 'a>(
    body: Element<'a, Message>,
    palette: Palette,
    _density: Density,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    container(body)
        .max_width(dialog_tokens::MAX_WIDTH)
        .width(Length::Shrink)
        .padding(Padding {
            top: dialog_tokens::H_PAD,
            right: dialog_tokens::H_PAD,
            bottom: dialog_tokens::H_PAD,
            left: dialog_tokens::H_PAD,
        })
        .style(move |_theme| container::Style {
            snap: false,
            background: Some(Background::Color(palette.raised.into_iced_color())),
            border: Border {
                color: palette.border.into_iced_color(),
                width: 1.0,
                radius: f32::from(radii.sm).into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::modal()),
            text_color: Some(palette.text.into_iced_color()),
        })
        .into()
}

/// CR-10 — dialog title row. 48 px tall, 18 sp Roboto weight-500
/// title text, 16 px horizontal padding.
pub fn dialog_title_row<'a, Message: 'a>(
    title: impl Into<String>,
    palette: Palette,
) -> Element<'a, Message> {
    container(
        text(title.into())
            .size(dialog_tokens::TITLE_FONT_SIZE)
            .font(Font {
                weight: iced::font::Weight::Medium,
                ..Font::DEFAULT
            })
            .color(palette.text.into_iced_color()),
    )
    .width(Length::Fill)
    .height(dialog_tokens::TITLE_ROW_HEIGHT)
    .padding(Padding {
        top: 0.0,
        right: dialog_tokens::H_PAD,
        bottom: 0.0,
        left: dialog_tokens::H_PAD,
    })
    .align_y(alignment::Vertical::Center)
    .into()
}

/// CR-10 — dialog button row. 64 px tall, right-aligned,
/// 8 px gap between buttons, 16 px horizontal padding.
/// Pass the action buttons in order (primary last — it renders
/// rightmost per the Classic ChromeOS "Primary right of Cancel"
/// spec).
pub fn dialog_button_row<'a, Message: 'a>(
    actions: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let spacer: Element<'a, Message> = Space::new()
        .width(Length::Fill)
        .height(Length::Shrink)
        .into();
    let mut items = vec![spacer];
    items.extend(actions);
    container(
        row(items)
            .spacing(dialog_tokens::BUTTON_GAP)
            .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .height(dialog_tokens::BUTTON_ROW_HEIGHT)
    .padding(Padding {
        top: 0.0,
        right: dialog_tokens::H_PAD,
        bottom: 0.0,
        left: dialog_tokens::H_PAD,
    })
    .into()
}

/// UX-9 (c) — dialog backdrop. A full-fill 50%-black surface
/// that sits below the dialog and intercepts clicks. Returns
/// just the container — pair with `iced::widget::stack` and
/// wire an `on_press` Message via `iced::mouse_area` if the
/// caller wants click-to-dismiss.
#[must_use]
pub fn dialog_backdrop<'a, Message: 'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            snap: false,
            background: Some(Background::Color(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: dialog_tokens::BACKDROP_OPACITY,
            })),
            ..container::Style::default()
        })
        .into()
}

/// UX-9 (d) — tooltip chrome. 12 sp text, SPACE_8 padding,
/// `Radii::sm` (4 px) corners, surface-3 (palette.overlay)
/// background. Fade-in timing (`Motion::tooltip_fade()`) lives
/// in the consumer's subscription wiring.
pub fn tooltip<'a, Message: 'a>(body: impl Into<String>, palette: Palette) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let sizes = FontSize::defaults();
    container(
        text(body.into())
            .size(TypeRole::Caption.size_in(sizes))
            .color(palette.text.into_iced_color()),
    )
    .padding(Padding {
        top: 6.0,
        right: 8.0,
        bottom: 6.0,
        left: 8.0,
    })
    .style(move |_theme| container::Style {
        snap: false,
        background: Some(Background::Color(palette.overlay.into_iced_color())),
        border: Border {
            color: palette.border.into_iced_color(),
            width: 1.0,
            radius: f32::from(radii.sm).into(),
        },
        shadow: mde_shadow_to_iced(MdeShadow::lift()),
        text_color: Some(palette.text.into_iced_color()),
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_theme::Density;

    #[test]
    fn outer_padding_resolves_to_lg2_at_comfortable() {
        let p = outer_padding(Density::Comfortable);
        // SPACE_24 = Space::lg2 = 24 px at comfortable.
        assert!((p.top - 24.0).abs() < 0.01);
        assert!((p.right - 24.0).abs() < 0.01);
        assert!((p.bottom - 24.0).abs() < 0.01);
        assert!((p.left - 24.0).abs() < 0.01);
    }

    #[test]
    fn outer_padding_scales_with_density() {
        let compact = outer_padding(Density::Compact);
        let comfortable = outer_padding(Density::Comfortable);
        let spacious = outer_padding(Density::Spacious);
        assert!(compact.top < comfortable.top);
        assert!(comfortable.top < spacious.top);
    }

    #[test]
    fn data_row_height_locked_to_ux6_minimum() {
        // UX-6 — 44 px row minimum.
        assert!((DATA_ROW_MIN_HEIGHT - 44.0).abs() < f32::EPSILON);
    }

    #[test]
    fn brighten_lightens_then_clamps() {
        let c = Color::from_rgb(0.5, 0.5, 0.5);
        let b = brighten(c, 1.5);
        assert!((b.r - 0.75).abs() < 0.001);
        // Clamp at 1.0.
        let max = brighten(Color::from_rgb(0.9, 0.9, 0.9), 2.0);
        assert!((max.r - 1.0).abs() < 0.001);
    }

    #[test]
    fn brighten_darkens_for_factor_below_one() {
        let c = Color::from_rgb(0.6, 0.6, 0.6);
        let d = brighten(c, 0.5);
        assert!((d.r - 0.3).abs() < 0.001);
    }

    #[test]
    fn badge_severity_variants_all_construct() {
        // Smoke — adding a new BadgeSeverity must update the
        // match arm in `status_badge`; otherwise the compiler
        // surfaces a non-exhaustive-match error here at build
        // time. Iterate every variant so the test fails to
        // compile if one is dropped.
        let palette = Palette::dark();
        let _ = status_badge::<()>("n", BadgeSeverity::Neutral, palette);
        let _ = status_badge::<()>("s", BadgeSeverity::Success, palette);
        let _ = status_badge::<()>("w", BadgeSeverity::Warning, palette);
        let _ = status_badge::<()>("d", BadgeSeverity::Danger, palette);
        let _ = status_badge::<()>("i", BadgeSeverity::Info, palette);
    }

    #[test]
    fn dialog_chrome_constructs_with_locked_tokens() {
        // UX-9 (c) — dialog builder must compile + apply the
        // locked tokens (480 px max-width, Radii::modal,
        // Shadow::modal). This test is a compile-time guard;
        // we can't introspect the resulting Element's style
        // fields from outside iced. The motion::dialog module's
        // tests guard the underlying token values directly.
        let palette = Palette::dark();
        let body: Element<'_, ()> = iced::widget::text("body").into();
        let _ = dialog::<()>(body, palette, Density::Comfortable);
        let _: Element<'_, ()> = dialog_backdrop();
        let _ = tooltip::<()>("hi", palette);
    }

    // ---- CR-3.b re-export smoke -------------------------------
    //
    // The full object_card body + 7 spec tests live at
    // crates/mde-iced-components/src/lib.rs after CR-3.b's extract;
    // panel_chrome re-exports the symbols so existing call sites
    // (mesh_topology and future CR-4..CR-8 consumers reaching
    // through panel_chrome) keep working. This test asserts the
    // re-export path resolves.

    #[test]
    fn object_card_reexport_resolves() {
        let palette = Palette::dark();
        let card = mde_theme::ObjectCard::small(mde_theme::Icon::Fleet, "smoke");
        let _: Element<'_, ()> = object_card(card, palette);
    }
}
