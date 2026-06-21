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
    Space as MdeSpace, StatusSeverity, TypeRole,
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

/// MOTION-NET-1 — map the dependency-free [`StatusSeverity`] tier (which the
/// canonical [`LoadState`] reports) onto the workbench's [`BadgeSeverity`], so
/// `status_badge` is the single badge renderer for both. Keeps the severity
/// decision in `mde-theme` (next to the state machine) while the toolkit dep
/// stays here.
#[must_use]
pub fn badge_severity(sev: StatusSeverity) -> BadgeSeverity {
    match sev {
        StatusSeverity::Neutral => BadgeSeverity::Neutral,
        StatusSeverity::Info => BadgeSeverity::Info,
        StatusSeverity::Success => BadgeSeverity::Success,
        StatusSeverity::Warning => BadgeSeverity::Warning,
        StatusSeverity::Danger => BadgeSeverity::Danger,
    }
}

/// MOTION-NET-1 — a compact, non-blocking **status pill** for a [`LoadState`]:
/// the state's status icon + its non-motion text [`label`](LoadState::label),
/// severity-tinted. This is the always-legible-without-motion affordance the
/// design doc's a11y acceptance requires — drop it in a panel header so the
/// async state is readable even with animation disabled (the spinner/shimmer
/// from later MOTION-NET items is the *motion* layer over this).
pub fn load_state_pill<'a, Message: 'a>(
    state: &LoadState,
    palette: Palette,
) -> Element<'a, Message> {
    let sizes = FontSize::defaults();
    let resolved = mde_icon(state.icon(), IconSize::Inline);
    let severity = badge_severity(state.severity());
    let fg = match severity {
        BadgeSeverity::Neutral => palette.text,
        BadgeSeverity::Success => palette.success,
        BadgeSeverity::Warning => palette.warning,
        BadgeSeverity::Danger => palette.danger,
        BadgeSeverity::Info => palette.accent,
    }
    .into_cosmic_color();

    let icon_el: Element<'a, Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(resolved.size_px()))
            .height(Length::Fixed(resolved.size_px()))
            .sty(move |_t: &cosmic::Theme| widget_svg::Style { color: Some(fg) })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(resolved.size_px())
            .colr(fg)
            .into()
    };

    let label = text(state.label())
        .size(TypeRole::Caption.size_in(sizes))
        .colr(fg)
        .align_y(alignment::Vertical::Center);

    let radii = Radii::defaults();
    let bg = match severity {
        BadgeSeverity::Neutral => palette.raised.into_cosmic_color(),
        BadgeSeverity::Info => palette.hover_tint().into_cosmic_color(),
        BadgeSeverity::Success => Color {
            a: 0.20,
            ..palette.success.into_cosmic_color()
        },
        BadgeSeverity::Warning => Color {
            a: 0.20,
            ..palette.warning.into_cosmic_color()
        },
        BadgeSeverity::Danger => Color {
            a: 0.20,
            ..palette.danger.into_cosmic_color()
        },
    };

    container(
        row![icon_el, label]
            .spacing(6)
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
        icon_color: Some(fg),
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

/// MOTION-NET-1 — the canonical **non-content chrome** for a [`LoadState`]:
/// the renderer a panel calls when [`LoadState::has_content`] is `false`, so
/// every surface paints the same distinct visual per state instead of
/// re-deriving the is-error-then-is-empty branch by hand:
///
///   * `Idle`     → a neutral "Nothing loaded yet" empty state.
///   * `Loading`  → a centered activity row (icon + "Loading…").
///   * `Offline`  → a warning empty state with a Retry CTA.
///   * `Failed`   → the danger [`error_state`] with the error detail + Retry.
///
/// Returns `None` for the content-bearing states (`Loaded` / `Degraded` /
/// `Refreshing{stale:true}`) — the panel renders its real data in those cases
/// (optionally topped with a [`load_state_pill`]). `on_retry` wires the CTA for
/// the recoverable states.
///
/// MOTION-NET-2 — the first-load `Loading` (and contentless `Refreshing`) arm
/// renders the shared [`skeleton`] placeholder (greyed Carbon blocks + shimmer
/// sweep) topped with the non-motion status pill, so a slow load shows
/// layout-shaped structure instead of a blank panel. `now` drives the shimmer
/// (advance it from a per-frame tick gated on the load being busy);
/// `reduce_motion` collapses the sweep to static grey.
pub fn load_state_chrome<'a, Message: Clone + 'a>(
    state: &LoadState,
    palette: Palette,
    density: Density,
    now: std::time::Instant,
    reduce_motion: bool,
    on_retry: impl Fn() -> Message + 'a,
) -> Option<Element<'a, Message>> {
    match state {
        // Content-bearing — the caller renders its data, not chrome.
        LoadState::Loaded | LoadState::Degraded | LoadState::Refreshing { stale: true } => None,
        LoadState::Refreshing { stale: false } | LoadState::Loading => {
            // MOTION-NET-2 — first load with nothing to show yet: the shared
            // skeleton + shimmer placeholder, topped with the non-motion status
            // pill (legible with animation disabled). The skeleton is the motion
            // layer over the MOTION-NET-1 pill.
            let pill = container(load_state_pill::<Message>(state, palette))
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Center);
            Some(
                column![
                    pill,
                    skeleton::<Message>(now, reduce_motion, palette, density),
                ]
                .spacing(f32::from(MdeSpace::for_density(density).md))
                .into(),
            )
        }
        LoadState::Idle => {
            let es = EmptyState::info(
                "Nothing loaded yet",
                "This panel hasn't loaded its data. Refresh to fetch it.",
            )
            .with_icon(Icon::StatusUnknown);
            Some(empty_state(es, palette, on_retry))
        }
        LoadState::Offline => {
            let es = EmptyState::with_cta(
                "Offline",
                "Can't reach the mesh right now. The panel will recover when \
                 connectivity returns — or retry now.",
                "Retry",
            )
            .with_icon(Icon::Wifi);
            Some(empty_state(es, palette, on_retry))
        }
        LoadState::Failed(err) => Some(error_state(err.clone(), palette, on_retry)),
    }
}

// ---- MOTION-NET-2 — shared skeleton + shimmer placeholders --------------

/// MOTION-NET-2 — number of placeholder rows the default skeleton paints. Enough
/// to fill a typical panel viewport so a slow load never shows blank space.
pub const SKELETON_ROW_COUNT: usize = 6;
/// MOTION-NET-2 — height of one skeleton block (px). Reads as a data row.
pub const SKELETON_BLOCK_HEIGHT: f32 = 16.0;
/// MOTION-NET-2 — vertical gap between skeleton rows (px).
pub const SKELETON_ROW_GAP: f32 = 12.0;

/// Channel-wise lerp between two themed colors (`t` clamped to `0.0..=1.0`).
/// Stays on palette tokens — no raw color constructor (§4).
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// MOTION-NET-2 — one greyed skeleton block with an animated shimmer sweep.
/// The block's fill is the `palette.raised` base lerped toward the lighter
/// `palette.overlay` highlight by the shimmer lift for its horizontal position
/// `pos` (`0.0..=1.0`) at cycle `phase`. Under `reduce_motion` the lift is a flat
/// `0.0` (static grey, no shimmer — the Q32 contract). `width`/`height` size the
/// block.
fn skeleton_block<'a, Message: 'a>(
    width: Length,
    height: f32,
    pos: f32,
    phase: f32,
    reduce_motion: bool,
    palette: Palette,
) -> Element<'a, Message> {
    let radii = Radii::defaults();
    let base = palette.raised.into_cosmic_color();
    let highlight = palette.overlay.into_cosmic_color();
    let lift = mde_theme::animation::shimmer_lift(phase, pos, reduce_motion);
    let fill = lerp_color(base, highlight, lift);
    container(Space::new().width(width).height(Length::Fixed(height)))
        .width(width)
        .height(Length::Fixed(height))
        .style(move |_theme| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(fill)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: f32::from(radii.sm).into(),
            },
            shadow: IcedShadow::default(),
            text_color: None,
        })
        .into()
}

/// MOTION-NET-2 — the shared **skeleton + shimmer** placeholder: a column of
/// greyed Carbon blocks with a shimmer highlight sweeping across each row, so a
/// slow first load shows layout-shaped structure instead of a blank panel or a
/// bare "Loading…" string. `now` drives the sweep via a
/// [`LoopingTween`](mde_theme::LoopingTween) over
/// [`SHIMMER_PERIOD_MS`](mde_theme::motion::list::SHIMMER_PERIOD_MS); under
/// `reduce_motion` the sweep is dropped and every block renders flat grey.
///
/// Each row's blocks are positioned along the placeholder width so the shimmer
/// reads as one continuous diagonal-free left→right sheen. The caller feeds a
/// `now` it advances from a per-frame tick (gated on the load being in flight,
/// so an idle panel runs no animation — MOTION-PERF-1).
pub fn skeleton<'a, Message: 'a>(
    now: std::time::Instant,
    reduce_motion: bool,
    palette: Palette,
    density: Density,
) -> Element<'a, Message> {
    use mde_theme::LoopingTween;
    let space = MdeSpace::for_density(density);
    // Anchor the looping clock to the process start so the phase is a pure
    // function of `now` (the consumer needn't store a start Instant).
    let period = std::time::Duration::from_millis(mde_theme::motion::list::SHIMMER_PERIOD_MS);
    let phase = LoopingTween::starting_at(shimmer_epoch(), period).phase(now);

    let mut col: Column<'a, Message, cosmic::Theme> = column![].spacing(SKELETON_ROW_GAP);
    for i in 0..SKELETON_ROW_COUNT {
        // A two-block row: a short "label" block + a longer "value" block,
        // mirroring `data_row`'s 40/60 rhythm so the skeleton matches the
        // eventual layout. Alternate the trailing width slightly so the rows
        // don't look like a perfect rectangle grid.
        let value_portion = if i % 2 == 0 { 50 } else { 35 };
        let row_el = row![
            skeleton_block(
                Length::Fixed(96.0),
                SKELETON_BLOCK_HEIGHT,
                0.12,
                phase,
                reduce_motion,
                palette,
            ),
            skeleton_block(
                Length::FillPortion(value_portion),
                SKELETON_BLOCK_HEIGHT,
                0.7,
                phase,
                reduce_motion,
                palette,
            ),
            Space::new().width(Length::FillPortion(100 - value_portion)),
        ]
        .spacing(f32::from(space.md))
        .align_y(alignment::Vertical::Center);
        col = col.push(row_el);
    }
    container(col)
        .width(Length::Fill)
        .padding(Padding {
            top: VERTICAL_PADDING,
            right: 24.0,
            bottom: VERTICAL_PADDING,
            left: 24.0,
        })
        .into()
}

/// Process-lifetime anchor for the shimmer's looping clock, so
/// [`skeleton`]'s phase depends only on the `now` the caller passes (no
/// per-panel start `Instant` to thread).
fn shimmer_epoch() -> std::time::Instant {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
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
    fn object_card_reexport_resolves() {
        let palette = crate::live_theme::palette();
        let card = mde_theme::ObjectCard::small(mde_theme::Icon::Fleet, "smoke");
        let _: Element<'_, ()> = object_card(card, palette);
    }

    // ---- MOTION-NET-1 — LoadState chrome -----------------------------

    #[test]
    fn badge_severity_maps_every_status_severity() {
        // Non-exhaustive match here fails to compile if a StatusSeverity
        // variant is dropped — the same compile-time guard the badge test uses.
        assert_eq!(
            badge_severity(StatusSeverity::Neutral),
            BadgeSeverity::Neutral
        );
        assert_eq!(badge_severity(StatusSeverity::Info), BadgeSeverity::Info);
        assert_eq!(
            badge_severity(StatusSeverity::Success),
            BadgeSeverity::Success
        );
        assert_eq!(
            badge_severity(StatusSeverity::Warning),
            BadgeSeverity::Warning
        );
        assert_eq!(
            badge_severity(StatusSeverity::Danger),
            BadgeSeverity::Danger
        );
    }

    #[test]
    fn load_state_chrome_renders_only_the_non_content_states() {
        // MOTION-NET-1 acceptance: each non-content state paints distinct
        // chrome; content-bearing states defer to the panel's own data view.
        let palette = crate::live_theme::palette();
        let density = Density::Comfortable;
        let now = std::time::Instant::now();
        let retry = || ();
        let chrome =
            |s: &LoadState| load_state_chrome::<()>(s, palette, density, now, false, retry);

        // Content-bearing → no chrome (panel renders its data).
        assert!(chrome(&LoadState::Loaded).is_none());
        assert!(chrome(&LoadState::Degraded).is_none());
        assert!(chrome(&LoadState::Refreshing { stale: true }).is_none());

        // Non-content → chrome.
        assert!(chrome(&LoadState::Idle).is_some());
        assert!(chrome(&LoadState::Loading).is_some());
        assert!(chrome(&LoadState::Offline).is_some());
        assert!(chrome(&LoadState::Failed("io".into())).is_some());
    }

    #[test]
    fn skeleton_renders_with_and_without_reduce_motion() {
        // MOTION-NET-2 — the shared skeleton constructs in both motion modes
        // (the pure shimmer-phase math is unit-tested in mde-theme::animation).
        let palette = crate::live_theme::palette();
        let now = std::time::Instant::now();
        let _: Element<'_, ()> = skeleton(now, false, palette, Density::Comfortable);
        let _: Element<'_, ()> = skeleton(now, true, palette, Density::Comfortable);
    }

    #[test]
    fn loading_chrome_carries_a_skeleton() {
        // MOTION-NET-2 acceptance — the Loading arm returns chrome (the
        // skeleton+pill), not None, so a slow load is never blank.
        let palette = crate::live_theme::palette();
        let now = std::time::Instant::now();
        let c = load_state_chrome::<()>(
            &LoadState::Loading,
            palette,
            Density::Comfortable,
            now,
            false,
            || (),
        );
        assert!(c.is_some(), "Loading must paint skeleton chrome");
    }

    #[test]
    fn load_state_pill_constructs_for_every_state() {
        let palette = crate::live_theme::palette();
        for s in [
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Refreshing { stale: true },
            LoadState::Loaded,
            LoadState::Degraded,
            LoadState::Offline,
            LoadState::Failed("x".into()),
        ] {
            let _: Element<'_, ()> = load_state_pill(&s, palette);
        }
    }
}
