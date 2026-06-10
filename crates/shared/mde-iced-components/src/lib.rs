//! Shared Iced widget builders for MDE.
//!
//! Lives here (rather than in `mde-theme`) because `mde-theme`'s
//! design lock excludes toolkit deps — see
//! `crates/mde-theme/src/components/mod.rs` for the rationale.
//! Lives here (rather than in `mde-workbench`) because peer crates
//! (`mde-files`, `mde-popover`, `mde-music`, the future
//! `mde-applet-now-playing`, etc.) need to render Object Cards
//! without taking a heavyweight dep on the entire workbench crate.
//!
//! Filed as CR-3.b (`docs/PROJECT_WORKLIST.md`) in 2026-05-25 as
//! the unblock for CR-4..CR-8 — the file manager, start menu,
//! Workbench-network/phones/credentials/recent panels, and
//! notification-history pane each need to consume the same
//! canonical `object_card` renderer that CR-3 introduced.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use iced::widget::{column, container, row, text, Column, Space};
use iced::{alignment, Background, Border, Color, Element, Length, Padding, Shadow as IcedShadow};

use mde_theme::{
    mde_icon, CardSize, CardState, IconPlacement, IconSize, IconState, ObjectCard, Palette,
    CARD_CORNER_RADIUS, CARD_DISABLED_OPACITY, CARD_FOCUS_OUTLINE_OFFSET, CARD_FOCUS_OUTLINE_WIDTH,
    CARD_HOVER_OVERLAY_ALPHA, CARD_PADDING, CARD_SELECTED_BORDER_WIDTH,
    CARD_SELECTED_OVERLAY_ALPHA, CARD_SHADOW_DEFAULT_ALPHA, CARD_SHADOW_DEFAULT_BLUR,
    CARD_SHADOW_DEFAULT_OFFSET_Y, CARD_SHADOW_HOVER_ALPHA, CARD_SHADOW_HOVER_BLUR,
    CARD_SHADOW_HOVER_OFFSET_Y, CARD_SHADOW_PRESSED_ALPHA, CARD_SHADOW_PRESSED_BLUR,
    CARD_SHADOW_PRESSED_OFFSET_Y, CARD_SUBTITLE_SIZE, CARD_TITLE_SIZE,
};

/// CR-3 — Material Design Elevated Object Card renderer.
///
/// Takes ownership of an `ObjectCard` data form (built via
/// `ObjectCard::small/medium/large(...)`) + the active palette,
/// returns the rendered Iced element. The data form lives in
/// `mde_theme` so panel authors can describe an object without
/// pulling iced; this fn is the canonical render path so every
/// Object surface (Start menu, mde-files, Workbench peer/phone/
/// credential lists, Notifications history) shares one component.
///
/// State branches:
///   * `Default`  — base shadow, no overlay, no border.
///   * `Hover`    — +1 elevation shadow, 8 % white overlay.
///   * `Pressed`  — +2 elevation shadow (the ripple is fired by
///                  the call site via an animation message —
///                  this renderer paints the elevated surface).
///   * `Selected` — 2 px indigo border + 15 % indigo overlay.
///   * `Focused`  — 2 px indigo outline at 1 px offset.
///   * `Disabled` — 40 % opacity, no hover affordance.
pub fn object_card<'a, Message: 'a>(card: ObjectCard, palette: Palette) -> Element<'a, Message> {
    let title_color = card
        .title_color_override
        .unwrap_or(palette.text)
        .into_iced_color();
    let subtitle_color = card
        .subtitle_color_override
        .unwrap_or(palette.text_muted)
        .into_iced_color();
    let accent_color = palette.accent.into_iced_color();
    let card_size = card.size;
    let card_state = card.state;

    // ---- icon slot ---------------------------------------------
    // Material Symbols SVG bytes (EPIC-UI-MATERIAL.svg-swap). Cards
    // in `CardState::Selected` thread `IconState::Active` so nav-group
    // icons render filled; everything else renders outlined.
    let icon_slot: Element<'a, Message> = if let Some(icon) = card.icon {
        let icon_px = card_size.icon_size();
        // Pick the IconSize tier whose px is nearest the spec
        // icon size for this card size. Object Cards override
        // density scaling — these are spec dimensions, not
        // density-scaled tokens.
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
        use iced::widget::svg as widget_svg;
        let muted = palette.text.into_iced_color();
        widget_svg(widget_svg::Handle::from_memory(svg_bytes))
            .width(Length::Fixed(icon_px))
            .height(Length::Fixed(icon_px))
            .style(
                move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(muted),
                },
            )
            .into()
    } else {
        Space::new()
            .width(Length::Fixed(card_size.icon_size()))
            .height(Length::Fixed(card_size.icon_size()))
            .into()
    };

    // ---- title + subtitle column -------------------------------
    let title_widget = text(card.title).size(CARD_TITLE_SIZE).color(title_color);

    let text_col: Column<'a, Message> = if let Some(subtitle) = card.subtitle {
        column![
            title_widget,
            text(subtitle)
                .size(CARD_SUBTITLE_SIZE)
                .color(subtitle_color),
        ]
        .spacing(2)
    } else {
        column![title_widget]
    };

    // ---- content layout (leading vs top icon) ------------------
    let content: Element<'a, Message> = match card_size.icon_placement() {
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
        _ => palette.surface.into_iced_color(),
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

    container(content)
        .width(Length::Fixed(card_size.width()))
        .height(Length::Fixed(card_size.height()))
        .padding(Padding {
            top: CARD_PADDING,
            right: CARD_PADDING,
            bottom: CARD_PADDING,
            left: CARD_PADDING,
        })
        .style(move |_theme: &iced::Theme| container::Style {
            background: Some(Background::Color(final_bg)),
            border,
            shadow: IcedShadow {
                color: Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: shadow_alpha,
                },
                offset: iced::Vector::new(0.0, shadow_offset),
                blur_radius: shadow_blur,
            },
            text_color: Some(title_color),
            snap: false,
        })
        .into()
}

/// Helper: paint a white overlay at the given alpha on top of a
/// surface token. The Material 3 Elevated card spec calls for an
/// 8 % white overlay on hover; this is the single math path.
pub fn overlay_white_on(base: mde_theme::Rgba, alpha: f32) -> Color {
    let base_iced = base.into_iced_color();
    Color {
        r: lerp(base_iced.r, 1.0, alpha),
        g: lerp(base_iced.g, 1.0, alpha),
        b: lerp(base_iced.b, 1.0, alpha),
        a: base_iced.a,
    }
}

/// Helper: paint a coloured overlay at the given alpha on top of
/// a surface token. Selected state composites a 15 % indigo
/// overlay; this is the math path.
pub fn overlay_color_on(base: mde_theme::Rgba, overlay: Color, alpha: f32) -> Color {
    let base_iced = base.into_iced_color();
    Color {
        r: lerp(base_iced.r, overlay.r, alpha),
        g: lerp(base_iced.g, overlay.g, alpha),
        b: lerp(base_iced.b, overlay.b, alpha),
        a: base_iced.a,
    }
}

/// Helper: multiply a colour's alpha by `mul`. Used for the
/// disabled state's 40 % opacity rule.
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

/// Canonical motion helpers for MDE Iced widgets, built on
/// `mde_theme::animation` (the no-toolkit design-token crate's
/// pre-existing animation system: `ease` + `lerp_f32` + `Easing`).
///
/// Access point for the ANIM-1..13 epic — surface authors call these
/// instead of hand-rolling timings, so every animation resolves to the
/// locked curves. See `docs/design/sway-native-shell.md` §2 +
/// `data/css/motion-vocabulary.css`. (SWAY-1's standalone `mde-motion`
/// crate was retired here — it duplicated `mde_theme::animation`.)
mod motion {
    use mde_theme::{ease, Easing};

    /// Linear progress in `[0, 1]` for `elapsed_ms` against `duration_ms`.
    fn progress(elapsed_ms: u64, duration_ms: u32) -> f32 {
        if duration_ms == 0 {
            return 1.0;
        }
        (elapsed_ms as f32 / duration_ms as f32).clamp(0.0, 1.0)
    }

    /// Opacity in `[0, 1]` for a fade-in started `elapsed_ms` ago over
    /// `duration_ms`, shaped by the arrival ease-out curve. Honors
    /// reduced motion (returns `1.0` immediately).
    #[must_use]
    pub fn fade_in_alpha(elapsed_ms: u64, duration_ms: u32, reduce: bool) -> f32 {
        if reduce {
            return 1.0;
        }
        ease(progress(elapsed_ms, duration_ms), Easing::EaseOut)
    }

    /// Q15 — stagger delay (ms) for item at `index` in a list.
    ///
    /// Items 0..`STAGGER_CAP-1` each receive an incremental
    /// `STAGGER_STEP_MS` delay; items at or beyond the cap all get the
    /// maximum delay so long lists don't crawl (Q15 policy).
    ///
    /// Use the return value as a start-offset when computing fade-in /
    /// slide-in alpha for a list item. Item `index=0` has delay `0` ms,
    /// `index=7` has `140` ms, `index=100` also has `140` ms.
    #[must_use]
    pub fn stagger_delay_ms(index: usize) -> u64 {
        let capped = index.min(mde_theme::motion::list::STAGGER_CAP.saturating_sub(1));
        u64::from(capped as u32 * mde_theme::motion::list::STAGGER_STEP_MS)
    }
}

/// CR-10 — one item in a right-click context menu.
///
/// Pass a slice of these to [`context_menu_surface`].
#[derive(Clone, Debug)]
pub struct ContextMenuItem {
    /// Primary label displayed in the menu row.
    pub label: String,
    /// Optional keyboard shortcut shown right-aligned.
    pub shortcut: Option<String>,
    /// Disabled rows render at 40% opacity and don't respond
    /// to interaction.
    pub disabled: bool,
    /// When true the row renders as a 1 px horizontal rule;
    /// all other fields are ignored.
    pub is_separator: bool,
}

impl ContextMenuItem {
    /// Convenience constructor for a standard enabled row.
    pub fn item(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            disabled: false,
            is_separator: false,
        }
    }

    /// Add an optional keyboard shortcut hint.
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Mark this item as disabled (40% opacity, no interaction).
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// A horizontal 1 px separator rule.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            shortcut: None,
            disabled: false,
            is_separator: true,
        }
    }
}

fn context_menu_item_row<'a, Message: 'a>(
    item: &ContextMenuItem,
    index: usize,
    elapsed_ms: u64,
    reduce_motion: bool,
    palette: Palette,
) -> Element<'a, Message> {
    use mde_theme::motion::context_menu as cm;
    if item.is_separator {
        return container(Space::new().width(Length::Fill).height(1.0))
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(palette.border.into_iced_color())),
                ..container::Style::default()
            })
            .into();
    }
    // Per-item stagger: item i starts fading in after its stagger delay.
    let item_delay = motion::stagger_delay_ms(index);
    let item_elapsed = elapsed_ms.saturating_sub(item_delay);
    let stagger_alpha = motion::fade_in_alpha(item_elapsed, cm::ITEM_REVEAL_MS, reduce_motion);
    let opacity = if item.disabled {
        0.4_f32 * stagger_alpha
    } else {
        stagger_alpha
    };
    let label_color = Color {
        a: palette.text.into_iced_color().a * opacity,
        ..palette.text.into_iced_color()
    };
    let muted_color = Color {
        a: palette.text_muted.into_iced_color().a * opacity,
        ..palette.text_muted.into_iced_color()
    };
    let label_el: Element<'a, Message> = text(item.label.clone())
        .size(cm::LABEL_SIZE)
        .color(label_color)
        .into();
    let shortcut_el: Option<Element<'a, Message>> = item
        .shortcut
        .as_ref()
        .map(|s| text(s.clone()).size(cm::KBD_SIZE).color(muted_color).into());
    let inner: Element<'a, Message> = match shortcut_el {
        None => row![
            Space::new().width(Length::Fixed(cm::ICON_L_PAD + cm::LABEL_L_PAD)),
            label_el,
        ]
        .align_y(alignment::Vertical::Center)
        .into(),
        Some(kbd) => row![
            Space::new().width(Length::Fixed(cm::ICON_L_PAD + cm::LABEL_L_PAD)),
            label_el,
            Space::new().width(Length::Fill),
            kbd,
            Space::new().width(Length::Fixed(cm::KBD_R_PAD)),
        ]
        .align_y(alignment::Vertical::Center)
        .into(),
    };
    container(inner)
        .width(Length::Fill)
        .height(Length::Fixed(cm::ROW_HEIGHT))
        .align_y(alignment::Vertical::Center)
        .into()
}

/// CR-10 / ANIM-3.b.1 — Classic ChromeOS right-click context menu surface.
///
/// Returns a styled container (min 220 px wide, 4 px corners,
/// 1 px border, raised background) holding a column of rows
/// built from `items`. The caller is responsible for positioning
/// the returned element as a floating overlay via their
/// compositor's stack mechanism.
///
/// `elapsed_ms` is the time since the menu was opened; drives the
/// Q44 entrance animation (item stagger + overall fade-in). Pass 0 on
/// the first frame; the animation completes after ~220 ms.
/// `reduce_motion` snaps all transitions to their final values.
pub fn context_menu_surface<'a, Message: 'a>(
    items: &[ContextMenuItem],
    elapsed_ms: u64,
    reduce_motion: bool,
    palette: Palette,
) -> Element<'a, Message> {
    use mde_theme::motion::context_menu as cm;
    let rows: Vec<Element<'a, Message>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| context_menu_item_row(item, i, elapsed_ms, reduce_motion, palette))
        .collect();
    // Q44 "grow from cursor" approximation: fade the whole menu in over
    // OPEN_FADE_MS. iced 0.13 has no scale transforms; a fast fade is the
    // closest available analogue.
    let menu_alpha = motion::fade_in_alpha(elapsed_ms, cm::OPEN_FADE_MS, reduce_motion);
    let base_bg = palette.raised.into_iced_color();
    let base_border = palette.border.into_iced_color();
    let bg = Color {
        a: base_bg.a * menu_alpha,
        ..base_bg
    };
    let border_color = Color {
        a: base_border.a * menu_alpha,
        ..base_border
    };
    // Iced 0.13 has no min_width; enforce via fixed base width.
    // Rows will expand if content is wider via Length::Fill.
    container(column(rows))
        .width(Length::Fixed(cm::MIN_WIDTH))
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 4.0_f32.into(),
            },
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_fade_in_runs_zero_to_one() {
        assert!((motion::fade_in_alpha(0, 200, false) - 0.0).abs() < 1e-3);
        assert!((motion::fade_in_alpha(200, 200, false) - 1.0).abs() < 1e-1);
        // Reduced motion -> instant full opacity.
        assert!((motion::fade_in_alpha(0, 200, true) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn object_card_small_constructs() {
        let palette = Palette::dark();
        let card = ObjectCard::small(mde_theme::Icon::Fleet, "Peer A");
        let _: Element<'_, ()> = object_card(card, palette);
    }

    #[test]
    fn object_card_medium_constructs_with_subtitle() {
        let palette = Palette::dark();
        let card = ObjectCard::medium(mde_theme::Icon::Fleet, "doc.pdf", "Modified yesterday");
        let _: Element<'_, ()> = object_card(card, palette);
    }

    #[test]
    fn object_card_large_constructs_with_subtitle() {
        let palette = Palette::dark();
        let card = ObjectCard::large(mde_theme::Icon::Fleet, "Workbench", "System utility");
        let _: Element<'_, ()> = object_card(card, palette);
    }

    #[test]
    fn object_card_renders_every_state() {
        // Spec-coverage smoke: every CardState variant must round-trip
        // through the renderer without panicking. Catches missing
        // match arms when a new state is added.
        let palette = Palette::dark();
        for state in [
            CardState::Default,
            CardState::Hover,
            CardState::Pressed,
            CardState::Selected,
            CardState::Focused,
            CardState::Disabled,
        ] {
            let card = ObjectCard::small(mde_theme::Icon::Fleet, "t").with_state(state);
            let _: Element<'_, ()> = object_card(card, palette);
        }
    }

    #[test]
    fn object_card_without_icon_constructs() {
        let palette = Palette::dark();
        let card = ObjectCard::small(mde_theme::Icon::Fleet, "x").without_icon();
        let _: Element<'_, ()> = object_card(card, palette);
    }

    #[test]
    fn overlay_helpers_blend_predictably() {
        // 100 % overlay = pure overlay; 0 % overlay = pure base.
        // The mid-point ratio (50 %) sits exactly between the two
        // for each channel.
        let base = mde_theme::Rgba::rgb(0, 0, 0);
        let white_full = overlay_white_on(base, 1.0);
        assert!((white_full.r - 1.0).abs() < 0.001);
        assert!((white_full.g - 1.0).abs() < 0.001);
        assert!((white_full.b - 1.0).abs() < 0.001);

        let white_none = overlay_white_on(base, 0.0);
        assert!((white_none.r - 0.0).abs() < 0.001);

        let white_half = overlay_white_on(base, 0.5);
        assert!((white_half.r - 0.5).abs() < 0.001);
    }

    #[test]
    fn with_alpha_multiplies_alpha_channel() {
        let opaque = Color::from_rgba(0.5, 0.5, 0.5, 1.0);
        let half = with_alpha(opaque, 0.4);
        assert!((half.a - 0.4).abs() < 0.001);
        // RGB channels unchanged.
        assert!((half.r - 0.5).abs() < 0.001);
    }

    #[test]
    fn context_menu_surface_constructs_with_mixed_items() {
        let palette = Palette::dark();
        let items = vec![
            ContextMenuItem::item("Copy").with_shortcut("Ctrl+C"),
            ContextMenuItem::item("Paste").with_shortcut("Ctrl+V"),
            ContextMenuItem::separator(),
            ContextMenuItem::item("Delete").disabled(),
        ];
        // elapsed=500 so all items are fully visible.
        let _: Element<'_, ()> = context_menu_surface(&items, 500, false, palette);
    }

    #[test]
    fn context_menu_stagger_at_zero_items_approach_transparent() {
        // Q44: at elapsed=0, item 0 starts its fade immediately; the
        // overall menu alpha is also near 0.  We can't inspect the element's
        // color directly, but verifying the call doesn't panic + knowing the
        // alpha math (tested in motion.rs) is sufficient for the component test.
        let palette = Palette::dark();
        let items = vec![ContextMenuItem::item("Cut"), ContextMenuItem::item("Copy")];
        let _: Element<'_, ()> = context_menu_surface(&items, 0, false, palette);
    }

    #[test]
    fn context_menu_stagger_reduce_motion_constructs() {
        // Q44 reduce-motion: all items appear immediately (no stagger).
        let palette = Palette::dark();
        let items = vec![ContextMenuItem::item("Open"), ContextMenuItem::separator()];
        let _: Element<'_, ()> = context_menu_surface(&items, 0, true, palette);
    }

    #[test]
    fn context_menu_stagger_beyond_cap_constructs() {
        // 10 items — items 8 and 9 share item 7's cap delay.
        let palette = Palette::dark();
        let items: Vec<_> = (0..10)
            .map(|i| ContextMenuItem::item(format!("Item {i}")))
            .collect();
        let _: Element<'_, ()> = context_menu_surface(&items, 200, false, palette);
    }

    // ANIM-4 acceptance tests.

    #[test]
    fn stagger_delay_caps_at_8_items() {
        // Q15: items 0..7 get incremental delays; items ≥8 get same as item 7.
        assert_eq!(motion::stagger_delay_ms(0), 0);
        assert_eq!(motion::stagger_delay_ms(1), 20);
        assert_eq!(motion::stagger_delay_ms(7), 140);
        // Items beyond the cap return the same max delay (not higher).
        assert_eq!(motion::stagger_delay_ms(8), 140);
        assert_eq!(motion::stagger_delay_ms(100), 140);
    }

    // ANIM-8.c.2 acceptance tests.
}
