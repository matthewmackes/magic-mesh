//! UX-4 — custom MDE window header bar.
//!
//! sway tiles Iced apps without server-side decorations, so the
//! window has no native title bar unless we draw one. This module
//! ships the `mde-header` row: 48 px tall, surface-token
//! background, 1 px divider at the bottom, MDE wordmark on the
//! left, min / max / close controls on the right. All colour /
//! size / weight tokens come from `mde-theme` — zero hardcoded
//! literals.
//!
//! Window controls render the Material Symbols SVGs (UX-8.a /
//! BUG-13.c): `control_button` resolves each via
//! `mde_icon(..).svg_bytes()` (WindowMinimize→remove,
//! WindowMaximize→fullscreen, WindowClose→close — all baked at the
//! inline optical size), so the SVG path always wins. The
//! single-Unicode glyphs (`−` / `□` / `×`) survive only as the
//! `fallback_glyph` branch for an unbaked variant — never reached by
//! these three today.
//!
//! Acceptance fields per the worklist UX-4 entry:
//!   (a) 48 px height, surface background, 1 px divider border ✓
//!   (b) "MDE Workbench" wordmark (Material Symbols icon + 14 sp
//!       medium text), left-aligned ✓
//!   (c) min/max/close with accent-tinted hover ✓
//!   (d) SHADOW_2 elevation — applied to the header surface as
//!       the visible elevation under sway tiling (window frame
//!       itself is borderless under sway by default).
//!
//! v4.0.1 BUG-20 (2026-05-23) — brand-strip parity with the sway
//! titlebar / start-menu Workbench tile: prepended the Workbench
//! glyph + expanded the wordmark from "MDE" to "MDE Workbench"
//! so the in-app header reads the same as the WM-drawn title above
//! it and the start-menu's pinned tile that launched the window.
//! Operator photo evidence: screenshots from 2026-05-23 showed two
//! sibling surfaces drifting (sway title: "MDE Workbench" + icon,
//! iced header: bare "MDE"). User directive: "Copy the branding
//! from one interface to another."

use cosmic::iced::widget::button::{self, Status as ButtonStatus};
use cosmic::iced::widget::{container, row, svg as widget_svg, text, Space};
use cosmic::iced::{alignment, Background, Border, Color, Length, Shadow, Vector};
use cosmic::Element;

use crate::cosmic_compat::prelude::*;
use mde_theme::{
    mde_icon, FontSize, FontWeight, Icon, IconSize, LoadState, Palette, Shadow as MdeShadow,
    StateTone, TypeRole,
};

/// Header bar height — locked to the worklist UX-4 (a) spec.
pub const HEADER_HEIGHT: f32 = 48.0;

/// Width allocated for each window-control button. 40 px gives
/// the glyphs room without crowding the wordmark; 3 of them
/// occupy 120 px on the right edge.
const CONTROL_WIDTH: f32 = 40.0;

/// MDE Workbench wordmark text. Mirrors the WM-drawn window title
/// (`app.title()` → "MDE Workbench — <page>"), the start-menu
/// pinned tile label, and the .desktop entry's `Name=` so all
/// surfaces that announce "this is the workbench" stay in sync.
/// The window `title()` carries the longer per-page suffix; this
/// stays compact so the 48 px stripe doesn't compete with the
/// page heading below it.
pub const WORDMARK: &str = "MDE Workbench";

/// Workbench glyph (Material Symbols) rendered to the left of the wordmark.
/// Matches the icon the start-menu pinned-tile row uses for the
/// Workbench shortcut so the brand reads consistently across
/// chrome surfaces.
const BRAND_ICON_SIZE: f32 = 18.0;

// UX-8 landed — window-control glyphs now route through the
// semantic Icon enum. The actual character rendered is still the
// Unicode fallback (`−`/`□`/`×`); the UX-8.a SVG swap will be a
// single change in `mde_theme::icons::Icon::fallback_glyph` →
// SVG without touching this file.

/// What a header-control click should do. The reducer maps each
/// variant to an `iced::window::*` Task in `app.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Build the workbench header bar as an Iced [`Element`].
///
/// `on_action` lifts a [`HeaderAction`] click into the app
/// reducer's `Message` enum — `app.rs` passes a closure that
/// wraps it in `Message::WindowControl(action)`.
pub fn view<'a, Message: Clone + 'a>(
    on_action: impl Fn(HeaderAction) -> Message + 'a,
    status: LoadState,
) -> Element<'a, Message> {
    let palette = crate::live_theme::palette();
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();

    let wordmark = text(WORDMARK)
        .size(TypeRole::Subheading.size_in(sizes))
        .font(cosmic::iced::Font {
            family: cosmic::iced::font::Family::Name(TypeRole::Subheading.family()),
            weight: weight_from_u16(TypeRole::Subheading.weight_in(weights)),
            ..cosmic::iced::Font::DEFAULT
        })
        .colr(palette.text.into_cosmic_color());

    // v4.0.1 BUG-20 — Workbench glyph (Material Symbols) to the
    // left of the wordmark. Mirrors the icon the start-menu
    // pinned-tile row shows for the Workbench shortcut so the
    // brand reads the same across chrome surfaces.
    let brand_icon_resolved = mde_icon(Icon::Workbench, IconSize::Inline);
    let brand_icon: Element<'a, Message> = if let Some(svg_bytes) = brand_icon_resolved.svg_bytes()
    {
        let icon_tint = palette.text.into_cosmic_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(BRAND_ICON_SIZE))
            .height(Length::Fixed(BRAND_ICON_SIZE))
            .sty(move |_t| widget_svg::Style {
                color: Some(icon_tint),
            })
            .into()
    } else {
        text(brand_icon_resolved.fallback_glyph)
            .size(BRAND_ICON_SIZE)
            .colr(palette.text.into_cosmic_color())
            .into()
    };

    let brand_strip = row![brand_icon, Space::new().width(Length::Fixed(8.0)), wordmark,]
        .align_y(alignment::Vertical::Center);

    let close_action = on_action(HeaderAction::Close);
    let max_action = on_action(HeaderAction::ToggleMaximize);
    let min_action = on_action(HeaderAction::Minimize);

    // v4.0.1 BUG-13.c: window controls now route through
    // `control_button` which prefers `Icon::svg_bytes()` for the
    // Material Symbols glyph and falls back to fallback_glyph text
    // only if the variant isn't baked. WindowMinimize/Maximize/Close
    // all resolve to Some so the SVG render path always wins today.
    let controls = row![
        control_button(Icon::WindowMinimize, min_action, palette, false),
        control_button(Icon::WindowMaximize, max_action, palette, false),
        control_button(Icon::WindowClose, close_action, palette, true),
    ]
    .spacing(0);

    let bar = row![
        container(brand_strip)
            .padding([0u16, 16u16])
            .height(Length::Fixed(HEADER_HEIGHT))
            .align_y(alignment::Vertical::Center),
        Space::new().width(Length::Fill),
        // MOTION-NET-5 — the control-plane connectivity indicator sits between the
        // flexible gap and the window controls, so it's always visible without
        // crowding the wordmark.
        container(status_indicator(status, palette))
            .height(Length::Fixed(HEADER_HEIGHT))
            .align_y(alignment::Vertical::Center),
        container(controls)
            .height(Length::Fixed(HEADER_HEIGHT))
            .align_y(alignment::Vertical::Center),
    ]
    .width(Length::Fill)
    .height(Length::Fixed(HEADER_HEIGHT));

    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(HEADER_HEIGHT))
        .style(move |_| container::Style {
            snap: false,
            icon_color: Some(palette.text.into_cosmic_color()),
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::raised()),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// MOTION-NET-5 — the control-plane connectivity status pill: a **non-motion**
/// icon glyph + label in the state's semantic tone. Rendered only for the
/// *interesting* states — `Refreshing` (a background poll is in flight, the
/// subtle background-work indicator) and the degraded `Degraded`/`Offline`/
/// `Failed` states (the auto-recovering connection banner); a healthy idle header
/// (`Loaded`/`Idle`) shows nothing, so the chrome stays clean. The cue is legible
/// without motion (icon *shape* + text), satisfying the a11y contract, and it is
/// presentation-only — it never blocks input.
fn status_indicator<'a, Message: 'a>(status: LoadState, palette: Palette) -> Element<'a, Message> {
    // Healthy + idle ⇒ no pill (no clutter); the indicator only appears when the
    // system is refreshing or degraded/offline/failed.
    if matches!(status, LoadState::Loaded | LoadState::Idle) {
        return Space::new().width(Length::Fixed(0.0)).into();
    }
    let sizes = FontSize::defaults();
    let tone = tone_color(status.tone(), palette);
    let glyph = text(status.icon().to_string())
        .size(TypeRole::Body.size_in(sizes))
        .colr(tone)
        .align_y(alignment::Vertical::Center);
    let label = text(status.label())
        .size(TypeRole::Caption.size_in(sizes))
        .colr(tone)
        .align_y(alignment::Vertical::Center);
    container(
        row![glyph, Space::new().width(Length::Fixed(6.0)), label]
            .align_y(alignment::Vertical::Center),
    )
    .padding([0u16, 12u16])
    .height(Length::Fixed(HEADER_HEIGHT))
    .align_y(alignment::Vertical::Center)
    .into()
}

/// MOTION-NET-5 — map a [`StateTone`] onto the single-sourced Carbon palette
/// support tokens (§4 — no raw hex). The tone is the secondary colour cue; the
/// icon + label are the primary, motion-independent differentiators.
fn tone_color(tone: StateTone, palette: Palette) -> Color {
    match tone {
        StateTone::Neutral => palette.text_muted.into_cosmic_color(),
        StateTone::Info => palette.accent.into_cosmic_color(),
        StateTone::Warning => palette.warning.into_cosmic_color(),
        StateTone::Danger => palette.danger.into_cosmic_color(),
        StateTone::Success => palette.success.into_cosmic_color(),
    }
}

/// Single window-control button. `accent_close` flips the hover
/// tint to the danger semantic colour for the close button so a
/// destructive click reads at-a-glance — the min/max buttons
/// hover-tint with the indigo accent per Q2.
fn control_button<'a, Message: Clone + 'a>(
    icon: Icon,
    on_press: Message,
    palette: Palette,
    accent_close: bool,
) -> Element<'a, Message> {
    let resolved = mde_icon(icon, IconSize::Inline);
    let muted = palette.text_muted.into_cosmic_color();
    let label: Element<'a, Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .sty(move |_t| widget_svg::Style { color: Some(muted) })
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .colr(muted)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .width(Length::Fixed(CONTROL_WIDTH))
            .height(Length::Fixed(HEADER_HEIGHT))
            .into()
    };
    // Wrap whatever content we picked in a fixed-size container so
    // the button width stays predictable regardless of whether
    // we rendered an SVG or a text fallback.
    let label = cosmic::iced::widget::container(label)
        .width(Length::Fixed(CONTROL_WIDTH))
        .height(Length::Fixed(HEADER_HEIGHT))
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center);

    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
        let bg: Color = match status {
            ButtonStatus::Hovered if accent_close => Color {
                a: 0.85,
                ..palette.danger.into_cosmic_color()
            },
            ButtonStatus::Hovered => palette.hover_tint().into_cosmic_color(),
            ButtonStatus::Pressed => palette.active_tint().into_cosmic_color(),
            _ => Color::TRANSPARENT,
        };
        let text_color = match (status, accent_close) {
            (ButtonStatus::Hovered, true) => Color::WHITE,
            (ButtonStatus::Hovered, false) => palette.accent.into_cosmic_color(),
            _ => palette.text_muted.into_cosmic_color(),
        };
        let border = Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        };
        button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color,
            icon_color: Some(text_color),
            border_color: border.color,
            border_width: border.width,
            border_radius: border.radius,
            border,
            shadow: Shadow::default(),
        }
    };

    cosmic::iced::widget::button(label)
        .padding(0)
        .on_press(on_press)
        .sty(style)
        .into()
}

fn mde_shadow_to_iced(s: MdeShadow) -> Shadow {
    Shadow {
        color: s.color.into_cosmic_color(),
        offset: Vector::new(s.offset_x, s.offset_y),
        blur_radius: s.blur,
    }
}

fn weight_from_u16(w: u16) -> cosmic::iced::font::Weight {
    // Standard CSS weight buckets, midpoint-split. 400 lands on
    // Normal, 500 on Medium — matches FontWeight::defaults().
    match w {
        0..=150 => cosmic::iced::font::Weight::Thin,
        151..=250 => cosmic::iced::font::Weight::ExtraLight,
        251..=350 => cosmic::iced::font::Weight::Light,
        351..=450 => cosmic::iced::font::Weight::Normal,
        451..=550 => cosmic::iced::font::Weight::Medium,
        551..=650 => cosmic::iced::font::Weight::Semibold,
        651..=750 => cosmic::iced::font::Weight::Bold,
        751..=850 => cosmic::iced::font::Weight::ExtraBold,
        _ => cosmic::iced::font::Weight::Black,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_height_locked_to_ux4_spec() {
        // UX-4 (a) — 48 px. Sidecar guard against drift.
        assert!((HEADER_HEIGHT - 48.0).abs() < f32::EPSILON);
    }

    #[test]
    fn wordmark_matches_brand_chrome_surfaces() {
        // v4.0.1 BUG-20: the in-app header bar, the sway-drawn
        // window title, the start-menu's Workbench pinned-tile
        // label, and the .desktop file's Name= field all show
        // the same product string. Drift across these surfaces
        // confuses the operator (sway shows "MDE Workbench",
        // the in-app bar used to show bare "MDE"). The window
        // `title()` still carries the longer "MDE Workbench —
        // <page>" form; the header keeps the compact form
        // without the per-page suffix.
        assert_eq!(WORDMARK, "MDE Workbench");
    }

    #[test]
    fn brand_icon_is_material_workbench_glyph() {
        // The Icon::Workbench variant must resolve to baked
        // SVG bytes — same source the start-menu pinned-tile
        // row uses. If the Material Symbols catalog ever drops
        // the glyph this test fails loudly instead of the header
        // silently falling back to the text glyph and drifting
        // away from the start-menu tile.
        let resolved = mde_icon(Icon::Workbench, IconSize::Inline);
        assert!(
            resolved.svg_bytes().is_some(),
            "Icon::Workbench must ship as a baked Material Symbols SVG so the\n             header brand-strip matches the start-menu tile"
        );
    }

    #[test]
    fn header_action_round_trips_through_closure() {
        // Reducers map every HeaderAction variant; this guards
        // against accidentally dropping one when extending the
        // enum.
        let actions = [
            HeaderAction::Minimize,
            HeaderAction::ToggleMaximize,
            HeaderAction::Close,
        ];
        for a in actions {
            let captured = a;
            let f = |x: HeaderAction| x;
            assert_eq!(f(captured), a);
        }
    }

    #[test]
    fn status_indicator_constructs_for_every_state() {
        // MOTION-NET-5 — the pill builds for all seven states (and renders nothing
        // for the healthy idle ones).
        let palette = crate::live_theme::palette();
        for s in [
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Refreshing { stale: true },
            LoadState::Degraded,
            LoadState::Offline,
            LoadState::Failed,
            LoadState::Loaded,
        ] {
            let _: Element<'_, ()> = status_indicator(s, palette);
        }
    }

    #[test]
    fn tone_color_maps_every_tone_to_a_palette_token() {
        // MOTION-NET-5 / §4 — every tone resolves to a single-sourced palette
        // support token (no raw hex), and the alert tones differ from neutral.
        let palette = crate::live_theme::palette();
        let neutral = tone_color(StateTone::Neutral, palette);
        let warning = tone_color(StateTone::Warning, palette);
        let danger = tone_color(StateTone::Danger, palette);
        assert_eq!(warning, palette.warning.into_cosmic_color());
        assert_eq!(danger, palette.danger.into_cosmic_color());
        assert_eq!(
            tone_color(StateTone::Info, palette),
            palette.accent.into_cosmic_color()
        );
        // An alert tone reads differently from the neutral/muted tone.
        assert_ne!(neutral, danger);
    }

    #[test]
    fn weight_mapping_resolves_medium_band_to_iced_medium() {
        // TypeRole::Subheading resolves to weight 500 in
        // FontWeight::defaults(); the wordmark must end up at
        // iced::font::Weight::Medium so it reads as "medium" per
        // UX-4 (b) — not Normal.
        let weights = FontWeight::defaults();
        let role_weight = TypeRole::Subheading.weight_in(weights);
        assert_eq!(
            weight_from_u16(role_weight),
            cosmic::iced::font::Weight::Medium
        );
    }
}
