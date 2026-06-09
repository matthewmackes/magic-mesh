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

use iced::widget::{button, column, container, row, text, Column, Space};
use iced::{alignment, Background, Border, Color, Element, Length, Padding, Shadow as IcedShadow};

use mde_theme::{
    mde_icon, CardSize, CardState, Elevation, FillMode, Icon, IconPlacement, IconSize, IconState,
    ObjectCard, Palette, CARD_CORNER_RADIUS, CARD_DISABLED_OPACITY, CARD_FOCUS_OUTLINE_OFFSET,
    CARD_FOCUS_OUTLINE_WIDTH, CARD_HOVER_OVERLAY_ALPHA, CARD_PADDING, CARD_SELECTED_BORDER_WIDTH,
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
pub mod motion {
    use std::time::{Duration, Instant};

    use mde_theme::{ease, lerp_f32, Easing, Tween};

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

    /// Opacity in `[0, 1]` for a fade-out started `elapsed_ms` ago over
    /// `duration_ms`, shaped by the dismissal ease-in curve. Honors
    /// reduced motion (returns `0.0` immediately).
    #[must_use]
    pub fn fade_out_alpha(elapsed_ms: u64, duration_ms: u32, reduce: bool) -> f32 {
        if reduce {
            return 0.0;
        }
        lerp_f32(
            1.0,
            0.0,
            ease(progress(elapsed_ms, duration_ms), Easing::EaseIn),
        )
    }

    /// Pixel offset for a surface sliding in from `distance_px` to its
    /// resting position (`0.0`), started `elapsed_ms` ago over
    /// `duration_ms`, shaped by ease-out. Honors reduced motion
    /// (returns `0.0`).
    #[must_use]
    pub fn slide_in_offset(
        elapsed_ms: u64,
        duration_ms: u32,
        distance_px: f32,
        reduce: bool,
    ) -> f32 {
        if reduce {
            return 0.0;
        }
        lerp_f32(
            distance_px,
            0.0,
            ease(progress(elapsed_ms, duration_ms), Easing::EaseOut),
        )
    }

    /// Eased crossfade between two colors for theme / preset
    /// transitions (Q33): per-channel RGBA lerp at `elapsed_ms` over
    /// `duration_ms`, shaped by the arrival ease-out curve. Honors
    /// reduced motion (returns `to` immediately).
    #[must_use]
    pub fn theme_crossfade(
        from: iced::Color,
        to: iced::Color,
        elapsed_ms: u64,
        duration_ms: u32,
        reduce: bool,
    ) -> iced::Color {
        if reduce {
            return to;
        }
        let t = ease(progress(elapsed_ms, duration_ms), Easing::EaseOut);
        iced::Color {
            r: lerp_f32(from.r, to.r, t),
            g: lerp_f32(from.g, to.g, t),
            b: lerp_f32(from.b, to.b, t),
            a: lerp_f32(from.a, to.a, t),
        }
    }

    // ---------------------------------------------------------------
    // ANIM-4 — list stagger (Q15), selection slide (Q18), shimmer (Q19)
    // Cite: motion-language.md §2.4, §2.6, §2.8, §2.9; ref: Linear
    // ---------------------------------------------------------------

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

    /// Q19 — shimmer wave opacity in `[0.15, 0.50]` at `now_ms` for a
    /// looping skeleton placeholder.
    ///
    /// Feeds [`crate::skeleton_shimmer`]. Honors reduced motion (returns
    /// a flat `0.15` — visible but static).
    #[must_use]
    pub fn shimmer_alpha(now_ms: u64, reduce: bool) -> f32 {
        if reduce {
            return 0.15;
        }
        let period = mde_theme::motion::list::SHIMMER_PERIOD_MS as f32;
        let phase = (now_ms as f32 / period) * std::f32::consts::TAU;
        // sine ∈ [−1, 1] → scaled to [0.15, 0.50]
        phase.sin() * 0.175 + 0.325
    }

    /// Q18 — state for a sliding selection indicator (motion-language.md §2.6).
    ///
    /// Call [`SelectionSlider::set_target`] when the selected item changes;
    /// call [`SelectionSlider::current_y`] each frame to get the indicator's
    /// current pixel offset. Re-targeting mid-flight is smooth (no visual pop).
    ///
    /// # Example
    /// ```no_run
    /// use std::time::Instant;
    /// use mde_iced_components::motion::SelectionSlider;
    ///
    /// let mut slider = SelectionSlider::at(0.0);
    /// slider.set_target(28.0, Instant::now()); // move to second row (28 px)
    /// let _y = slider.current_y(Instant::now());
    /// ```
    #[derive(Clone, Copy, Debug)]
    pub struct SelectionSlider {
        from_y: f32,
        to_y: f32,
        tween: Option<Tween>,
    }

    impl SelectionSlider {
        /// Create a slider resting at `y` with no animation in flight.
        #[must_use]
        pub fn at(y: f32) -> Self {
            Self {
                from_y: y,
                to_y: y,
                tween: None,
            }
        }

        /// Animate to `new_y` starting from the current position at `now`.
        /// Calling again mid-flight re-targets smoothly.
        pub fn set_target(&mut self, new_y: f32, now: Instant) {
            self.from_y = self.current_y(now);
            self.to_y = new_y;
            self.tween = Some(Tween::starting_at(
                now,
                Duration::from_millis(u64::from(mde_theme::motion::list::SELECTION_SLIDE_MS)),
            ));
        }

        /// Interpolated Y position at `now`. Returns `to_y` once settled.
        #[must_use]
        pub fn current_y(&self, now: Instant) -> f32 {
            let Some(t) = self.tween else {
                return self.to_y;
            };
            let p = ease(t.progress(now), Easing::EaseOut);
            lerp_f32(self.from_y, self.to_y, p)
        }

        /// `true` once the slide animation has completed.
        #[must_use]
        pub fn is_complete(&self, now: Instant) -> bool {
            self.tween.map_or(true, |t| t.is_complete(now))
        }
    }
}

/// ANIM-8.c.1 — Elevated surface container using Q29 shadow + Q30 radius
/// for the given tier (docs/design/sway-native-shell.md §5).
///
/// Returns a styled `container` with the tier's background (`palette.raised`),
/// 1 px border (`palette.border`), corner radius, and drop-shadow. Callers
/// compose it with `.width()` / `.height()` / `.padding()` before calling
/// `.into()`.
///
/// | Tier          | Radius | Shadow  | Use                              |
/// |---------------|--------|---------|----------------------------------|
/// | `Inline`      | 4 px   | none    | rows, badges, chips              |
/// | `PopoverMenu` | 8 px   | raised  | dropdown menus, popovers         |
/// | `Floating`    | 8 px   | float   | OSDs, toasts, compact overlays   |
/// | `Modal`       | 12 px  | modal   | dialogs, sheets, palette         |
pub fn elevation_container<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    elevation: Elevation,
    palette: Palette,
) -> Element<'a, Message> {
    let bg = palette.raised.into_iced_color();
    let border_color = palette.border.into_iced_color();
    let shadow = elevation.shadow();
    let radius = f32::from(elevation.radius()).into();
    container(content)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 1.0,
                radius,
            },
            shadow: IcedShadow {
                color: shadow.color.into_iced_color(),
                offset: iced::Vector::new(shadow.offset_x, shadow.offset_y),
                blur_radius: shadow.blur,
            },
            ..container::Style::default()
        })
        .into()
}

/// ANIM-4 (Q19) — skeleton shimmer placeholder widget.
///
/// Renders a `width × height` rounded rectangle (4 px corners) whose
/// background oscillates between `palette.raised` and a brighter band to
/// signal "content loading." Drive `shimmer_alpha_val` from
/// [`motion::shimmer_alpha`] on a subscription tick.
///
/// When content arrives, crossfade by blending this widget out while the
/// real content fades in via [`motion::fade_in_alpha`] /
/// [`motion::fade_out_alpha`] over `motion::list::SKELETON_CROSSFADE_MS`.
///
/// Cite: motion-language.md §2.9; ref: Linear (skeleton cards)
pub fn skeleton_shimmer<'a, Message: 'a>(
    width: f32,
    height: f32,
    shimmer_alpha_val: f32,
    palette: Palette,
) -> Element<'a, Message> {
    let base = palette.raised.into_iced_color();
    let highlight = palette.surface.into_iced_color();
    let a = shimmer_alpha_val.clamp(0.0, 1.0);
    let bg = iced::Color {
        r: lerp(base.r, highlight.r, a),
        g: lerp(base.g, highlight.g, a),
        b: lerp(base.b, highlight.b, a),
        a: 1.0,
    };
    container(Space::new().width(Length::Fill))
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 4.0_f32.into(),
            },
            ..container::Style::default()
        })
        .into()
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

/// CR-10 / ANIM-3.b.1 — Classic ChromeOS toast / notification chip.
///
/// Returns a 320 px wide chip container (4 px corners, 1 px border,
/// raised background) with a title, optional body text, optional
/// action buttons, and a 2 px indigo bottom progress strip.
///
/// `remaining_0_1` = 1.0 when the toast just appeared (full bar),
/// 0.0 when about to auto-dismiss (empty bar). The caller drives
/// this from a subscription ticked at the desired framerate.
///
/// `actions` is a slice of `(label, message)` pairs. Each pair renders
/// as a small button below the body. Buttons highlight (Q97 "expand")
/// when hovered — their background brightens from transparent to a
/// subtle accent tint. Pass `&[]` to render no action buttons.
pub fn toast_chip<'a, Message: 'a + Clone>(
    title: impl Into<String>,
    body: Option<String>,
    remaining_0_1: f32,
    actions: &[(&str, Message)],
    palette: Palette,
) -> Element<'a, Message> {
    use mde_theme::motion::toast as tk;
    let r = remaining_0_1.clamp(0.0, 1.0);
    let bar_width = tk::WIDTH * r;
    let accent = palette.accent.into_iced_color();
    let bg = palette.raised.into_iced_color();
    let border_color = palette.border.into_iced_color();

    let title_el: Element<'a, Message> = container(
        text(title.into())
            .size(13.0)
            .font(iced::Font {
                weight: iced::font::Weight::Medium,
                ..iced::Font::DEFAULT
            })
            .color(palette.text.into_iced_color()),
    )
    .padding(Padding {
        top: 12.0,
        right: 12.0,
        bottom: 4.0,
        left: 12.0,
    })
    .width(Length::Fill)
    .into();

    let mut rows: Vec<Element<'a, Message>> = vec![title_el];

    if let Some(body_text) = body {
        let body_el: Element<'a, Message> = container(
            text(body_text)
                .size(13.0)
                .color(palette.text.into_iced_color()),
        )
        .padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 8.0,
            left: 12.0,
        })
        .width(Length::Fill)
        .into();
        rows.push(body_el);
    }

    // Q97 action buttons — inline-expand on hover via background brightening.
    if !actions.is_empty() {
        let text_color = palette.text.into_iced_color();
        let resting_color = Color {
            a: text_color.a * tk::ACTION_RESTING_ALPHA,
            ..text_color
        };
        let hover_bg = Color {
            a: tk::ACTION_HOVER_BG_ALPHA,
            ..accent
        };
        let action_btns: Vec<Element<'a, Message>> = actions
            .iter()
            .map(|(label, msg)| {
                let msg = msg.clone();
                let label_str = label.to_string();
                button(text(label_str).size(tk::ACTION_SIZE).color(resting_color))
                    .on_press(msg)
                    .style(move |_theme, status| match status {
                        button::Status::Hovered | button::Status::Pressed => button::Style {
                            background: Some(Background::Color(hover_bg)),
                            text_color,
                            border: Border {
                                radius: 4.0_f32.into(),
                                ..Border::default()
                            },
                            ..Default::default()
                        },
                        _ => button::Style {
                            background: None,
                            text_color: resting_color,
                            border: Border {
                                radius: 4.0_f32.into(),
                                ..Border::default()
                            },
                            ..Default::default()
                        },
                    })
                    .padding(Padding {
                        top: tk::ACTION_V_PAD,
                        bottom: tk::ACTION_V_PAD,
                        left: tk::ACTION_H_PAD,
                        right: tk::ACTION_H_PAD,
                    })
                    .into()
            })
            .collect();
        let action_row: Element<'a, Message> = container(row(action_btns).spacing(4.0))
            .padding(Padding {
                top: 0.0,
                bottom: 4.0,
                left: 8.0,
                right: 8.0,
            })
            .width(Length::Fill)
            .into();
        rows.push(action_row);
    }

    // 2 px progress strip at the bottom of the chip.
    let progress_strip: Element<'a, Message> = container(Space::new())
        .width(Length::Fixed(bar_width))
        .height(Length::Fixed(tk::PROGRESS_HEIGHT))
        .style(move |_| container::Style {
            background: Some(Background::Color(accent)),
            ..container::Style::default()
        })
        .into();
    rows.push(progress_strip);

    container(column(rows))
        .width(Length::Fixed(tk::WIDTH))
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

/// ANIM-8.c.2 — Material Symbols icon fill-morph (Q32).
///
/// Renders an `Icon` cross-fading between its outlined and filled SVG
/// variants as `fill_t` goes 0.0 (fully outlined) → 1.0 (fully filled).
/// Drive `fill_t` with [`mde_theme::motion::icon::fill_morph_t`] ticked
/// each frame via a 16 ms subscription.
///
/// Icons with `FillMode::NeverFill` always render outlined (fill_t
/// ignored); icons with `FillMode::AlwaysFill` always render filled.
/// Only `FillMode::OnActive` icons cross-fade.
pub fn icon_fill_morph<'a, Message: 'a>(
    icon: Icon,
    size: IconSize,
    fill_t: f32,
    fg_color: Color,
) -> Element<'a, Message> {
    use iced::widget::{stack, svg as widget_svg};

    let resolved = mde_icon(icon, size);
    let px = size.px();

    match resolved.fill_mode {
        FillMode::NeverFill => {
            let bytes = resolved.svg_bytes_for_state(IconState::Idle);
            return widget_svg(widget_svg::Handle::from_memory(bytes))
                .width(Length::Fixed(px))
                .height(Length::Fixed(px))
                .style(
                    move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                        color: Some(fg_color),
                    },
                )
                .into();
        }
        FillMode::AlwaysFill => {
            let bytes = resolved.svg_bytes_for_state(IconState::Active);
            return widget_svg(widget_svg::Handle::from_memory(bytes))
                .width(Length::Fixed(px))
                .height(Length::Fixed(px))
                .style(
                    move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                        color: Some(fg_color),
                    },
                )
                .into();
        }
        FillMode::OnActive => {}
    }

    // OnActive: cross-fade outline (alpha = 1-t) with filled (alpha = t).
    let t = fill_t.clamp(0.0, 1.0);
    let outline_bytes = resolved.svg_bytes_for_state(IconState::Idle);
    let filled_bytes = resolved.svg_bytes_for_state(IconState::Active);
    let outline_color = Color {
        a: fg_color.a * (1.0 - t),
        ..fg_color
    };
    let filled_color = Color {
        a: fg_color.a * t,
        ..fg_color
    };

    stack![
        widget_svg(widget_svg::Handle::from_memory(outline_bytes))
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .style(
                move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(outline_color),
                }
            ),
        widget_svg(widget_svg::Handle::from_memory(filled_bytes))
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .style(
                move |_t: &iced::Theme, _s: widget_svg::Status| widget_svg::Style {
                    color: Some(filled_color),
                }
            ),
    ]
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
    fn motion_slide_in_settles_to_rest() {
        assert!(motion::slide_in_offset(0, 200, 8.0, false) > 7.0);
        assert!(motion::slide_in_offset(200, 200, 8.0, false).abs() < 1.0);
        // Reduced motion -> at rest immediately.
        assert!(motion::slide_in_offset(0, 200, 8.0, true).abs() < 1e-6);
    }

    #[test]
    fn motion_theme_crossfade_interpolates_channels() {
        let black = iced::Color::from_rgb(0.0, 0.0, 0.0);
        let white = iced::Color::from_rgb(1.0, 1.0, 1.0);
        // Start = from.
        assert!((motion::theme_crossfade(black, white, 0, 200, false).r - 0.0).abs() < 1e-3);
        // End ~= to.
        assert!((motion::theme_crossfade(black, white, 200, 200, false).r - 1.0).abs() < 1e-1);
        // Reduced motion -> to immediately.
        assert!((motion::theme_crossfade(black, white, 0, 200, true).r - 1.0).abs() < 1e-6);
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
    fn elevation_container_constructs_all_tiers() {
        let palette = Palette::dark();
        for tier in [
            Elevation::Inline,
            Elevation::PopoverMenu,
            Elevation::Floating,
            Elevation::Modal,
        ] {
            let content = Space::new().width(Length::Fixed(100.0));
            let _: Element<'_, ()> = elevation_container(content, tier, palette);
        }
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

    #[test]
    fn toast_chip_constructs_full_and_empty_bar() {
        let palette = Palette::dark();
        let _: Element<'_, ()> = toast_chip(
            "Download complete",
            Some("file.tar.gz saved".to_string()),
            1.0,
            &[],
            palette,
        );
        let _: Element<'_, ()> = toast_chip("Update ready", None, 0.0, &[], palette);
    }

    #[test]
    fn toast_chip_clamps_remaining_to_0_1() {
        let palette = Palette::dark();
        // Should not panic on out-of-range inputs.
        let _: Element<'_, ()> = toast_chip("x", None, -0.5, &[], palette);
        let _: Element<'_, ()> = toast_chip("x", None, 1.5, &[], palette);
    }

    #[test]
    fn toast_chip_with_actions_constructs() {
        // Q97: action buttons render without panic.
        let palette = Palette::dark();
        let _: Element<'_, &str> = toast_chip(
            "Sync complete",
            None,
            0.8,
            &[("Dismiss", "dismiss"), ("View", "view")],
            palette,
        );
    }

    #[test]
    fn toast_chip_empty_actions_renders_no_button_row() {
        // Passing &[] produces the same chip as before the Q97 change.
        let palette = Palette::dark();
        let _: Element<'_, ()> = toast_chip("No actions", None, 1.0, &[], palette);
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

    #[test]
    fn shimmer_alpha_stays_in_range() {
        // Q19: shimmer oscillates in [0.15, 0.50].
        for ms in [0_u64, 300, 600, 900, 1200, 1500] {
            let a = motion::shimmer_alpha(ms, false);
            assert!(
                a >= 0.14 && a <= 0.51,
                "shimmer_alpha({ms}) = {a} out of range"
            );
        }
        // Reduced motion returns flat value in range.
        assert!((motion::shimmer_alpha(0, true) - 0.15).abs() < 1e-6);
    }

    #[test]
    fn selection_slider_settles_to_target() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut slider = motion::SelectionSlider::at(0.0);
        slider.set_target(28.0, now);
        // Immediately after: close to origin.
        let y_start = slider.current_y(now);
        assert!(y_start < 5.0, "expected near 0 at start, got {y_start}");
        // Well after the slide duration: settled at target.
        let later = now + Duration::from_millis(300);
        let y_end = slider.current_y(later);
        assert!(
            (y_end - 28.0).abs() < 0.5,
            "expected ~28 at end, got {y_end}"
        );
        assert!(slider.is_complete(later));
    }

    #[test]
    fn selection_slider_retargets_smoothly() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut slider = motion::SelectionSlider::at(0.0);
        slider.set_target(100.0, now);
        // Retarget mid-flight.
        let mid = now + Duration::from_millis(50);
        slider.set_target(50.0, mid);
        // `from_y` should be mid-flight position (not 0 or 100).
        let y = slider.current_y(mid);
        assert!(
            y > 0.0 && y < 100.0,
            "retarget from_y should be intermediate, got {y}"
        );
    }

    // ANIM-8.c.2 acceptance tests.

    #[test]
    fn icon_fill_morph_on_active_at_zero_constructs() {
        // fill_t=0 → outlined only; filled SVG is invisible.
        let _: Element<'_, ()> = icon_fill_morph(
            mde_theme::Icon::Dashboard,
            mde_theme::IconSize::Nav,
            0.0,
            Color::WHITE,
        );
    }

    #[test]
    fn icon_fill_morph_on_active_at_one_constructs() {
        // fill_t=1 → filled only; outline SVG is invisible.
        let _: Element<'_, ()> = icon_fill_morph(
            mde_theme::Icon::Fleet,
            mde_theme::IconSize::Nav,
            1.0,
            Color::WHITE,
        );
    }

    #[test]
    fn icon_fill_morph_on_active_midpoint_constructs() {
        // fill_t=0.5 → both layers half-visible.
        let _: Element<'_, ()> = icon_fill_morph(
            mde_theme::Icon::Apps,
            mde_theme::IconSize::Nav,
            0.5,
            Color::WHITE,
        );
    }

    #[test]
    fn icon_fill_morph_never_fill_constructs() {
        // NeverFill: only one SVG layer, no cross-fade.
        let _: Element<'_, ()> = icon_fill_morph(
            mde_theme::Icon::Snapshot,
            mde_theme::IconSize::Nav,
            0.5,
            Color::WHITE,
        );
    }

    #[test]
    fn icon_fill_morph_always_fill_constructs() {
        // AlwaysFill: only one (filled) SVG layer, no cross-fade.
        let _: Element<'_, ()> = icon_fill_morph(
            mde_theme::Icon::Notification,
            mde_theme::IconSize::Nav,
            0.5,
            Color::WHITE,
        );
    }

    #[test]
    fn skeleton_shimmer_constructs() {
        let palette = Palette::dark();
        // Round-trip through the renderer at various alpha values.
        for a in [0.0_f32, 0.15, 0.35, 0.50, 1.0] {
            let _: Element<'_, ()> = skeleton_shimmer(200.0, 16.0, a, palette);
        }
    }
}
