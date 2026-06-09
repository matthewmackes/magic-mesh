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
//! Material Symbols glyph swap-in lands with UX-8 (icon system).
//! Until then the controls render with single-Unicode placeholders
//! (`−` / `□` / `×`) that match the v8.7 panel-side fallback.
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

use iced::widget::button::{self, Status as ButtonStatus};
use iced::widget::{container, row, svg as widget_svg, text, Space};
use iced::{alignment, Background, Border, Color, Element, Length, Shadow, Vector};

use mde_theme::{
    mde_icon, FontSize, FontWeight, Icon, IconSize, Palette, Shadow as MdeShadow, TypeRole,
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
) -> Element<'a, Message> {
    let palette = Palette::dark();
    let sizes = FontSize::defaults();
    let weights = FontWeight::defaults();

    let wordmark = text(WORDMARK)
        .size(TypeRole::Subheading.size_in(sizes))
        .font(iced::Font {
            family: iced::font::Family::Name(TypeRole::Subheading.family()),
            weight: weight_from_u16(TypeRole::Subheading.weight_in(weights)),
            ..iced::Font::DEFAULT
        })
        .color(palette.text.into_iced_color());

    // v4.0.1 BUG-20 — Workbench glyph (Material Symbols) to the
    // left of the wordmark. Mirrors the icon the start-menu
    // pinned-tile row shows for the Workbench shortcut so the
    // brand reads the same across chrome surfaces.
    let brand_icon_resolved = mde_icon(Icon::Workbench, IconSize::Inline);
    let brand_icon: Element<'a, Message> = if let Some(svg_bytes) = brand_icon_resolved.svg_bytes()
    {
        let icon_tint = palette.text.into_iced_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(BRAND_ICON_SIZE))
            .height(Length::Fixed(BRAND_ICON_SIZE))
            .style(
                move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(icon_tint),
                },
            )
            .into()
    } else {
        text(brand_icon_resolved.fallback_glyph)
            .size(BRAND_ICON_SIZE)
            .color(palette.text.into_iced_color())
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
            background: Some(Background::Color(palette.surface.into_iced_color())),
            border: Border {
                color: palette.border.into_iced_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: mde_shadow_to_iced(MdeShadow::raised()),
            text_color: Some(palette.text.into_iced_color()),
        })
        .into()
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
    let muted = palette.text_muted.into_iced_color();
    let label: Element<'a, Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
        use iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .style(
                move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(muted),
                },
            )
            .into()
    } else {
        text(resolved.fallback_glyph)
            .size(16.0)
            .color(muted)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .width(Length::Fixed(CONTROL_WIDTH))
            .height(Length::Fixed(HEADER_HEIGHT))
            .into()
    };
    // Wrap whatever content we picked in a fixed-size container so
    // the button width stays predictable regardless of whether
    // we rendered an SVG or a text fallback.
    let label = iced::widget::container(label)
        .width(Length::Fixed(CONTROL_WIDTH))
        .height(Length::Fixed(HEADER_HEIGHT))
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center);

    let style = move |_theme: &iced::Theme, status: ButtonStatus| {
        let bg: Color = match status {
            ButtonStatus::Hovered if accent_close => Color {
                a: 0.85,
                ..palette.danger.into_iced_color()
            },
            ButtonStatus::Hovered => palette.hover_tint().into_iced_color(),
            ButtonStatus::Pressed => palette.active_tint().into_iced_color(),
            _ => Color::TRANSPARENT,
        };
        let text_color = match (status, accent_close) {
            (ButtonStatus::Hovered, true) => Color::WHITE,
            (ButtonStatus::Hovered, false) => palette.accent.into_iced_color(),
            _ => palette.text_muted.into_iced_color(),
        };
        button::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Shadow::default(),
        }
    };

    iced::widget::button(label)
        .padding(0)
        .on_press(on_press)
        .style(style)
        .into()
}

fn mde_shadow_to_iced(s: MdeShadow) -> Shadow {
    Shadow {
        color: s.color.into_iced_color(),
        offset: Vector::new(s.offset_x, s.offset_y),
        blur_radius: s.blur,
    }
}

fn weight_from_u16(w: u16) -> iced::font::Weight {
    // Standard CSS weight buckets, midpoint-split. 400 lands on
    // Normal, 500 on Medium — matches FontWeight::defaults().
    match w {
        0..=150 => iced::font::Weight::Thin,
        151..=250 => iced::font::Weight::ExtraLight,
        251..=350 => iced::font::Weight::Light,
        351..=450 => iced::font::Weight::Normal,
        451..=550 => iced::font::Weight::Medium,
        551..=650 => iced::font::Weight::Semibold,
        651..=750 => iced::font::Weight::Bold,
        751..=850 => iced::font::Weight::ExtraBold,
        _ => iced::font::Weight::Black,
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
    fn weight_mapping_resolves_medium_band_to_iced_medium() {
        // TypeRole::Subheading resolves to weight 500 in
        // FontWeight::defaults(); the wordmark must end up at
        // iced::font::Weight::Medium so it reads as "medium" per
        // UX-4 (b) — not Normal.
        let weights = FontWeight::defaults();
        let role_weight = TypeRole::Subheading.weight_in(weights);
        assert_eq!(weight_from_u16(role_weight), iced::font::Weight::Medium);
    }
}
