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

use cosmic::iced::widget::button::Status as ButtonStatus;
use cosmic::iced::widget::{button, column, container, row, text, Column, Space};
use cosmic::iced::{
    alignment, Background, Border, Color, Font, Length, Padding, Shadow as IcedShadow,
};
// CUT-1: cosmic::Element bakes in cosmic::Theme, matching the theme the
// .colr()/.sty() compat widgets thread through the tree.
use cosmic::Element;

use crate::cosmic_compat::prelude::*;

use mde_theme::{
    components::empty_state::{BODY_CTA_GAP, EMPTY_ICON_SIZE, HEADING_BODY_GAP, VERTICAL_PADDING},
    mde_icon,
    motion::dialog as dialog_tokens,
    Density, EmptyState, FontSize, Icon, IconSize, LoadState, Palette, Radii, Shadow as MdeShadow,
    Space as MdeSpace, StateTone, TypeRole,
};

// CR-3.b — `object_card` extracted to `mde-iced-components` so
// peer crates (mde-files, mde-popover, mde-music, etc.) can render
// Object Cards without taking a heavyweight dep on mde-workbench.
// Re-exported here so existing workbench call sites stay
// unchanged.
pub use crate::cosmic_compat::object_card;

/// UX-6 — minimum data-row height. Component dimension, not
/// density-scaled.
pub const DATA_ROW_MIN_HEIGHT: f32 = 44.0;

/// CV-3 — the standard heading↔body / section column gap
/// (`space.lg`, 20 px at Comfortable), density-aware so Compact /
/// Spacious modes scale the gap in step with `outer_padding`.
pub fn column_gap(density: Density) -> f32 {
    f32::from(MdeSpace::for_density(density).lg)
}

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
        .colr(palette.text.into_cosmic_color())
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
        .colr(palette.text_muted.into_cosmic_color())
        .align_y(alignment::Vertical::Center)
        .width(Length::FillPortion(40));
    let value_text = text(value.into())
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color())
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
            palette.raised.into_cosmic_color(),
            palette.text.into_cosmic_color(),
        ),
        BadgeSeverity::Success => (
            Color {
                a: 0.20,
                ..palette.success.into_cosmic_color()
            },
            palette.success.into_cosmic_color(),
        ),
        BadgeSeverity::Warning => (
            Color {
                a: 0.20,
                ..palette.warning.into_cosmic_color()
            },
            palette.warning.into_cosmic_color(),
        ),
        BadgeSeverity::Danger => (
            Color {
                a: 0.20,
                ..palette.danger.into_cosmic_color()
            },
            palette.danger.into_cosmic_color(),
        ),
        BadgeSeverity::Info => (
            palette.hover_tint().into_cosmic_color(),
            palette.accent.into_cosmic_color(),
        ),
    };

    container(
        text(label.into())
            .size(TypeRole::Caption.size_in(sizes))
            .colr(fg)
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
        icon_color: None,
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

/// MOTION-NET-1 — map the semantic [`StateTone`] onto the panel-chrome
/// [`BadgeSeverity`] fill. Both are the shared Carbon severity ramp, so the
/// mapping is 1:1.
#[must_use]
pub const fn badge_severity_for(tone: StateTone) -> BadgeSeverity {
    match tone {
        StateTone::Neutral => BadgeSeverity::Neutral,
        StateTone::Info => BadgeSeverity::Info,
        StateTone::Warning => BadgeSeverity::Warning,
        StateTone::Danger => BadgeSeverity::Danger,
        StateTone::Success => BadgeSeverity::Success,
    }
}

/// MOTION-NET-1 — the canonical async-state indicator: a pill badge showing a
/// [`LoadState`]'s distinct icon glyph + label, severity-tinted from its
/// [`StateTone`]. The glyph + text carry the state (legible without motion and
/// without relying on colour); the tint is a secondary cue. This is the shared
/// renderer panels use instead of ad-hoc "Working…" strings, so every surface
/// reads async state the same way.
pub fn load_state_indicator<'a, Message: 'a>(
    state: LoadState,
    palette: Palette,
) -> Element<'a, Message> {
    status_badge(
        format!("{} {}", state.icon(), state.label()),
        badge_severity_for(state.tone()),
        palette,
    )
}

/// MOTION-NET-5 — how many back-to-back failed polls a panel tolerates before it
/// declares the connection **degraded**. Below this a single hiccup is absorbed
/// silently (the next poll usually succeeds); at/above it the surface admits the
/// connection is unhealthy. One short blip shouldn't flap the indicator.
pub const CONNECTION_DEGRADED_AFTER: u32 = 2;

/// MOTION-NET-5 — derive the canonical [`LoadState`] for a panel from its raw
/// connectivity inputs, so every panel reads *background-poll* and
/// *connection-degraded* the same way instead of hand-rolling the busy→state
/// mapping (`mesh_services` grew its own three-arm version).
///
/// The mapping, in precedence order:
///
/// * **Background poll** (`polling == true`):
///   * with content already on screen ⇒ [`LoadState::Refreshing { stale: true }`]
///     — a *non-blocking* refresh (keep the old data, dim it, show the indicator),
///     never a blank panel;
///   * with nothing yet ⇒ [`LoadState::Loading`] — the blocking first load.
/// * **Connection degraded** (`consecutive_failures >= CONNECTION_DEGRADED_AFTER`,
///   not currently polling): the bus/mesh is unreachable —
///   * with cached content still on screen ⇒ [`LoadState::Degraded`] (usable, but
///     stale — flagged);
///   * with nothing cached ⇒ [`LoadState::Offline`] (no data, waiting to reconnect).
/// * **Settled**: `has_content` ⇒ [`LoadState::Loaded`]; otherwise
///   [`LoadState::Idle`].
///
/// **Auto-recovery** falls out for free: a panel resets `consecutive_failures` to
/// `0` on any successful poll, so the very next derivation drops straight back to
/// `Loaded`/`Idle` — the degraded/offline state clears itself the instant the mesh
/// answers again, with no separate "recover" path to forget to call. Pure; the
/// caller owns the counter (bump on a failed poll, clear on success).
#[must_use]
pub fn connection_state(polling: bool, has_content: bool, consecutive_failures: u32) -> LoadState {
    if polling {
        return if has_content {
            LoadState::Refreshing { stale: true }
        } else {
            LoadState::Loading
        };
    }
    if consecutive_failures >= CONNECTION_DEGRADED_AFTER {
        return if has_content {
            LoadState::Degraded
        } else {
            LoadState::Offline
        };
    }
    if has_content {
        LoadState::Loaded
    } else {
        LoadState::Idle
    }
}

/// MOTION-NET-5 — the **non-blocking** background-activity indicator: a small,
/// unobtrusive "Updating…" pill shown ONLY while a panel is polling in the
/// background over content that's already on screen
/// (`state == Refreshing { stale: true }`). Unlike [`load_state_indicator`] — which
/// renders the full async-state badge for any state, including the blocking
/// `Loading` — this surfaces *only* the case the operator must see without being
/// interrupted: "work is happening, your data is still good". For every other
/// state it renders an empty zero-size element, so dropping it into a header costs
/// nothing when the panel isn't doing background work.
///
/// The glyph (`↻`) + "Updating…" text carry the meaning without motion or colour
/// (the a11y contract); the `Info` tint is the secondary cue. The indicator never
/// blocks: it sits beside the content, which stays visible (dimmed via
/// [`LoadState::content_alpha`]), rather than replacing it with a spinner.
pub fn background_activity_indicator<'a, Message: 'a>(
    state: LoadState,
    palette: Palette,
) -> Element<'a, Message> {
    if state == (LoadState::Refreshing { stale: true }) {
        status_badge(
            format!("{} Updating…", LoadState::Refreshing { stale: true }.icon()),
            badge_severity_for(StateTone::Info),
            palette,
        )
    } else {
        Space::new()
            .width(Length::Shrink)
            .height(Length::Shrink)
            .into()
    }
}

/// MOTION-NET-2 — a layout-matching skeleton placeholder: `rows` greyed Carbon
/// bars (≈ a data row tall) shown while a panel is `LoadState::Loading`, so a
/// slow first load never shows a blank area or a bare "Loading…" string. The
/// grey shimmers via `animation::shimmer_alpha(phase, reduce_motion)`; under
/// reduce-motion it renders a STATIC grey (no shimmer). Swap to the real content
/// when data lands (shimmer stops). `rows` = the expected row count of the
/// eventual layout.
pub fn skeleton<'a, Message: 'a>(
    rows: usize,
    palette: Palette,
    phase: f32,
    reduce_motion: bool,
) -> Element<'a, Message> {
    let alpha = mde_theme::animation::shimmer_alpha(phase, reduce_motion);
    let bar = Color {
        a: alpha,
        ..palette.text_muted.into_cosmic_color()
    };
    let radii = Radii::defaults();
    let mut col: Column<'a, Message, cosmic::Theme> = column![].spacing(8);
    for _ in 0..rows {
        col = col.push(
            container(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(DATA_ROW_MIN_HEIGHT - 12.0)),
            )
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                snap: false,
                icon_color: None,
                background: Some(Background::Color(bar)),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: f32::from(radii.sm).into(),
                },
                shadow: IcedShadow::default(),
                text_color: None,
            }),
        );
    }
    col.into()
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
            icon_color: None,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: f32::from(radii.md).into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::lift()),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// EFF-45 — error-state renderer: the load-FAILED counterpart to
/// [`empty_state`], so a panel whose data source errored never
/// masquerades as "nothing to show yet". Same layout family
/// (icon · heading · body · CTA) but unambiguous failure styling:
/// [`Icon::StatusError`], a fixed "Couldn't load this panel"
/// heading, the error detail danger-tinted, and a Retry CTA.
///
/// Pattern: panel state carries `load_error: Option<String>`; its
/// loader message is a `Result` (never silently mapped to an empty
/// vec); `view()` checks `load_error` BEFORE the is-empty branch.
pub fn error_state<'a, Message: Clone + 'a>(
    detail: impl Into<String>,
    palette: Palette,
    on_retry: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    let mut state = EmptyState::with_cta("Couldn't load this panel", detail.into(), "Retry")
        .with_icon(Icon::StatusError);
    state.body_color_override = Some(palette.danger);
    empty_state(state, palette, on_retry)
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
        .into_cosmic_color();

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
            use cosmic::iced::widget::svg as widget_svg;
            let muted = palette.text_muted.into_cosmic_color();
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(resolved.size_px()))
                .height(Length::Fixed(resolved.size_px()))
                .sty(move |_t: &cosmic::Theme| widget_svg::Style { color: Some(muted) })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(resolved.size_px())
                .colr(palette.text_muted.into_cosmic_color())
                .align_x(alignment::Horizontal::Center)
                .into()
        }
    } else {
        Space::new().height(Length::Fixed(EMPTY_ICON_SIZE)).into()
    };
    let heading = text(state.heading)
        .size(TypeRole::Heading.size_in(sizes))
        .colr(palette.text.into_cosmic_color())
        .align_x(alignment::Horizontal::Center);
    let body = text(state.body)
        .size(TypeRole::Body.size_in(sizes))
        .colr(body_color)
        .align_x(alignment::Horizontal::Center);

    let mut col: Column<'a, Message, cosmic::Theme> = column![icon_slot, heading, body]
        .spacing(HEADING_BODY_GAP)
        .align_x(alignment::Horizontal::Center);

    if let Some(label) = state.cta_label {
        let accent_color = palette.accent.into_cosmic_color();
        let radii = Radii::defaults();
        let cta_button: Element<'a, Message> = button(
            text(label)
                .size(TypeRole::Body.size_in(sizes))
                .colr(Color::WHITE),
        )
        .padding(Padding {
            top: 8.0,
            right: 20.0,
            bottom: 8.0,
            left: 20.0,
        })
        .on_press(on_cta())
        .sty(move |_theme, status: ButtonStatus| {
            let bg = match status {
                ButtonStatus::Hovered => brighten(accent_color, 1.10),
                ButtonStatus::Pressed => brighten(accent_color, 0.90),
                _ => accent_color,
            };
            button::Style {
                snap: false,
                icon_color: None,
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: f32::from(radii.md).into(),
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
        color: s.color.into_cosmic_color(),
        offset: cosmic::iced::Vector::new(s.offset_x, s.offset_y),
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
            icon_color: None,
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: f32::from(radii.sm).into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::modal()),
            text_color: Some(palette.text.into_cosmic_color()),
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
                weight: cosmic::iced::font::Weight::Medium,
                ..Font::DEFAULT
            })
            .colr(palette.text.into_cosmic_color()),
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
            // Pure-black backdrop scrim token; opacity is the dialog token.
            background: Some(Background::Color(Color {
                a: dialog_tokens::BACKDROP_OPACITY,
                ..mde_theme::carbon::BLACK.into_cosmic_color()
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
            .colr(palette.text.into_cosmic_color()),
    )
    .padding(Padding {
        top: 6.0,
        right: 8.0,
        bottom: 6.0,
        left: 8.0,
    })
    .style(move |_theme| container::Style {
        snap: false,
        icon_color: None,
        background: Some(Background::Color(palette.overlay.into_cosmic_color())),
        border: Border {
            color: palette.border.into_cosmic_color(),
            width: 1.0,
            radius: f32::from(radii.sm).into(),
        },
        shadow: mde_shadow_to_iced(MdeShadow::lift()),
        text_color: Some(palette.text.into_cosmic_color()),
    })
    .into()
}

/// PLANES-2 / H8 — the installed version of a distro **package**, from
/// `rpm -q <pkg>` (the NVR string), or `None` when it isn't installed —
/// the uniform, honest version source for hero captions. Call from a
/// panel's `load()` (it shells out), never from `view()`.
#[must_use]
pub fn pkg_version(pkg: &str) -> Option<String> {
    let out = std::process::Command::new("rpm")
        .args(["-q", pkg])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() || s.contains("not installed") {
        None
    } else {
        Some(s)
    }
}

/// PLANES-2 — process-lifetime memo of [`pkg_version`] so a panel can
/// caption its hero straight from `view()` without threading a version
/// field through its load/state. The first lookup per package shells
/// `rpm -q` once; every later call (and every repaint) is a map hit.
#[must_use]
pub fn pkg_version_cached(pkg: &str) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<String>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(pkg) {
            return hit.clone();
        }
    }
    let v = pkg_version(pkg);
    if let Ok(mut map) = cache.lock() {
        map.insert(pkg.to_string(), v.clone());
    }
    v
}

/// PLANES-2 (H3/H4/H8/H9/H10) — the primary-service **hero band**: a
/// service's monochrome line-art (tinted with the single `HERO_STROKE`
/// Carbon token, §4 — H6/H7) at 112 px, captioned with the service NAME
/// (H8) and a live version, or an honest "not installed" when the
/// service is absent (the art always renders — H10). Hovering reveals a
/// small stack card (H9). A primary-service panel drops this into its
/// header, aligned right (H3/H4).
pub fn hero_band<'a, Message: 'a>(
    hero: mde_theme::hero::Hero,
    version: Option<&str>,
    palette: Palette,
) -> Element<'a, Message> {
    let art = cosmic::iced::widget::svg(cosmic::iced::widget::svg::Handle::from_memory(
        hero.svg_bytes(),
    ))
    .width(Length::Fixed(112.0))
    .height(Length::Fixed(112.0))
    .sty(|_t: &cosmic::Theme| cosmic::iced::widget::svg::Style {
        color: Some(mde_theme::hero::HERO_STROKE.into_cosmic_color()),
    });
    let caption = match version {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => "not installed".to_string(),
    };
    let band = column![
        art,
        text(hero.name())
            .size(13)
            .colr(palette.text.into_cosmic_color()),
        text(caption.clone())
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(2)
    .align_x(alignment::Horizontal::Center);
    // H9 — hover reveals a small "stack card": the service's role in the
    // MCNF platform + its version line.
    let card = tooltip(
        format!(
            "{} — {caption}\npart of the MCNF platform stack",
            hero.name()
        ),
        palette,
    );
    cosmic::iced::widget::tooltip(band, card, cosmic::iced::widget::tooltip::Position::Bottom)
        .gap(6.0)
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
        let c = Color::from_rgb(0.5, 0.5, 0.5); // carbon-ok: test fixture (not a render-path token)
        let b = brighten(c, 1.5);
        assert!((b.r - 0.75).abs() < 0.001);
        // Clamp at 1.0.
        let max = brighten(Color::from_rgb(0.9, 0.9, 0.9), 2.0); // carbon-ok: test fixture
        assert!((max.r - 1.0).abs() < 0.001);
    }

    #[test]
    fn brighten_darkens_for_factor_below_one() {
        let c = Color::from_rgb(0.6, 0.6, 0.6); // carbon-ok: test fixture
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
        let palette = crate::live_theme::palette();
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
        let palette = crate::live_theme::palette();
        let body: Element<'_, ()> = cosmic::iced::widget::text("body")
            .colr(palette.text.into_cosmic_color())
            .into();
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
    fn load_state_indicator_renders_every_state() {
        // MOTION-NET-1 — the renderer constructs for all 7 states (the badge's
        // glyph + label carry the distinction; the tone-mapped fill is secondary).
        let palette = crate::live_theme::palette();
        for s in [
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Refreshing { stale: true },
            LoadState::Refreshing { stale: false },
            LoadState::Degraded,
            LoadState::Offline,
            LoadState::Failed,
            LoadState::Loaded,
        ] {
            let _: Element<'_, ()> = load_state_indicator(s, palette);
        }
        // The tone→severity map is total (a missing arm would fail to compile).
        for t in [
            StateTone::Neutral,
            StateTone::Info,
            StateTone::Warning,
            StateTone::Danger,
            StateTone::Success,
        ] {
            let _ = badge_severity_for(t);
        }
    }

    #[test]
    fn connection_state_distinguishes_background_poll_from_blocking_first_load() {
        // MOTION-NET-5 — a poll *with* content on screen is a non-blocking
        // background refresh (keep the stale data); a poll with nothing yet is the
        // blocking first load.
        assert_eq!(
            connection_state(true, true, 0),
            LoadState::Refreshing { stale: true },
            "polling over content ⇒ non-blocking background refresh"
        );
        assert_eq!(
            connection_state(true, false, 0),
            LoadState::Loading,
            "polling with no content yet ⇒ blocking first load"
        );
        // A poll-in-progress takes precedence over the failure count: while we are
        // actively retrying, show "refreshing", not "degraded".
        assert_eq!(
            connection_state(true, true, 9),
            LoadState::Refreshing { stale: true },
            "an in-flight poll outranks a stale failure count"
        );
    }

    #[test]
    fn connection_state_flags_degraded_then_offline_when_the_mesh_is_unreachable() {
        // MOTION-NET-5 — below the threshold a single blip is absorbed (still
        // Loaded); at/above it the connection is declared degraded (cached content
        // kept) or offline (nothing cached to show).
        assert_eq!(
            connection_state(false, true, CONNECTION_DEGRADED_AFTER - 1),
            LoadState::Loaded,
            "one blip below the threshold doesn't flap to degraded"
        );
        assert_eq!(
            connection_state(false, true, CONNECTION_DEGRADED_AFTER),
            LoadState::Degraded,
            "repeated failures with cached content ⇒ degraded (usable, flagged)"
        );
        assert_eq!(
            connection_state(false, false, CONNECTION_DEGRADED_AFTER),
            LoadState::Offline,
            "repeated failures with nothing cached ⇒ offline"
        );
        // The degraded/offline states both offer a reconnect affordance.
        assert!(connection_state(false, true, CONNECTION_DEGRADED_AFTER).can_retry());
        assert!(connection_state(false, false, CONNECTION_DEGRADED_AFTER).can_retry());
    }

    #[test]
    fn connection_state_auto_recovers_when_a_poll_succeeds() {
        // MOTION-NET-5 — the degraded/offline state clears itself the instant the
        // mesh answers again: a successful poll resets the failure counter to 0, so
        // the next derivation drops straight back to Loaded/Idle with no separate
        // "recover" call.
        let degraded = connection_state(false, true, CONNECTION_DEGRADED_AFTER);
        assert_eq!(degraded, LoadState::Degraded);
        // Caller clears the counter on success → auto-recovered.
        assert_eq!(
            connection_state(false, true, 0),
            LoadState::Loaded,
            "failures reset ⇒ back to Loaded"
        );
        assert_eq!(
            connection_state(false, false, 0),
            LoadState::Idle,
            "failures reset, nothing loaded ⇒ Idle"
        );
    }

    #[test]
    fn background_activity_indicator_shows_only_for_a_non_blocking_refresh() {
        // MOTION-NET-5 — the non-blocking indicator renders for exactly the
        // background-poll-over-content case and is an empty no-op element for every
        // other state (so it never blocks or interrupts).
        let palette = crate::live_theme::palette();
        // Constructs (and is a real badge) for the one surfaced state…
        let _: Element<'_, ()> =
            background_activity_indicator(LoadState::Refreshing { stale: true }, palette);
        // …and is a harmless empty element for everything else (blocking Loading
        // included — that's the skeleton/load_state_indicator's job, not this one).
        for s in [
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Refreshing { stale: false },
            LoadState::Degraded,
            LoadState::Offline,
            LoadState::Failed,
            LoadState::Loaded,
        ] {
            let _: Element<'_, ()> = background_activity_indicator(s, palette);
        }
    }

    #[test]
    fn skeleton_constructs_for_any_row_count_and_reduce_motion() {
        // MOTION-NET-2 — the skeleton renders for an empty, single, and
        // multi-row layout, both shimmering and static (reduce-motion).
        let palette = crate::live_theme::palette();
        for rows in [0usize, 1, 5, 12] {
            let _: Element<'_, ()> = skeleton(rows, palette, 0.3, false);
            let _: Element<'_, ()> = skeleton(rows, palette, 0.3, true); // reduce-motion
        }
    }

    #[test]
    fn object_card_reexport_resolves() {
        let palette = crate::live_theme::palette();
        let card = mde_theme::ObjectCard::small(mde_theme::Icon::Fleet, "smoke");
        let _: Element<'_, ()> = object_card(card, palette);
    }
}
