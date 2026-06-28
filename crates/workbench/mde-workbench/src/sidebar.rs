//! Iced sidebar widget.
//!
//! CB-1.2 lock: collapsible per-group rows. Pure-data
//! [`SidebarState`] (which groups are expanded, which row is
//! focused for keyboard navigation) lives here so the reducer
//! tests can stay Iced-free; the actual `view()` builder pulls
//! in Iced widgets.
//!
//! **UNIFY-7 (2026-06-28)** — the nav is restyled to the Unified
//! Workbench design's dense collapsible grouped nav
//! (`docs/design/workbench/Workbench.dc.html` lines 46-65 / the
//! `nav()` model lines 537-547): a `#262626` (`surface`) panel,
//! uppercase group headers carrying a `+`/`–` collapse chevron,
//! indented items each prefixed by a 5 px status pip, and the
//! active item drawn with a 3 px accent left-border + `raised`
//! surface fill. Every colour is an IBM Carbon token from
//! `live_theme::palette()` (§4); the status pip reuses
//! `panel_chrome::status_dot_sized`. Component dimensions
//! (sidebar width, row height, stripe width) are NOT
//! density-scaled — UX-24 requires density to scale spacing only.

use cosmic::iced::widget::button::Status as ButtonStatus;
use cosmic::iced::widget::{button, column, container, row, text, Column, Space};
use cosmic::iced::{alignment, Background, Border, Color, Element, Font, Length, Padding, Shadow};

use mde_theme::Palette;

use crate::cosmic_compat::prelude::*;
use crate::keyboard::Pane;
use crate::model::{nav_model, Group, View};

/// UX-5 (a) — 240 px fixed sidebar width. Component dimension,
/// not density-scaled. (Design aside is 230 px; the 240 px lock
/// is kept — the 10 px delta is below the dense-nav threshold.)
pub const SIDEBAR_WIDTH: f32 = 240.0;

/// UNIFY-7 — dense nav-item / group-header row height. Tightened
/// from the old UX-5 40 px to the design's compact rows.
pub const NAV_ROW_HEIGHT: f32 = 28.0;

/// UNIFY-7 (design line 624) — 3 px accent left-border on the
/// active item's left edge (was UX-5's 2 px stripe).
const SELECTED_STRIPE_WIDTH: f32 = 3.0;

/// UX-5 (f) — focus-ring border width on the active row when the
/// sidebar pane holds keyboard focus.
const FOCUS_RING_WIDTH: f32 = 2.0;

/// UNIFY-7 (design line 624) — nav-item label point size (13 sp,
/// down from UX-5's 14 sp). Not density-scaled per UX-24.
const NAV_LABEL_SIZE: f32 = 13.0;

/// UX-5 (e) / UNIFY-7 — uppercase group-header label point size.
const SECTION_LABEL_SIZE: f32 = 11.0;

/// UNIFY-7 (design line 53) — `+`/`–` collapse-chevron point size.
const CHEVRON_SIZE: f32 = 14.0;

/// UNIFY-7 (design line 625) — per-item status pip diameter (5 px).
const NAV_DOT_DIAMETER: f32 = 5.0;

/// Per-group expand/collapse + focus state. The active group
/// (matching the current [`View`]) is always expanded
/// automatically — additional groups can be toggled by the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidebarState {
    /// Groups the user explicitly toggled open in addition to
    /// the active one. The active group never appears here —
    /// it's implicitly expanded.
    user_expanded: Vec<Group>,
}

impl SidebarState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Is `group` currently expanded? True when it's the active
    /// group or the user toggled it open.
    #[must_use]
    pub fn is_expanded(&self, group: Group, active: Group) -> bool {
        group == active || self.user_expanded.contains(&group)
    }

    /// Toggle the user-expanded state of `group`. No-op when
    /// `group` is the active one (which is always expanded).
    pub fn toggle(&mut self, group: Group, active: Group) {
        if group == active {
            return;
        }
        if let Some(idx) = self.user_expanded.iter().position(|g| *g == group) {
            self.user_expanded.remove(idx);
        } else {
            self.user_expanded.push(group);
        }
    }
}

/// Build the sidebar tree for an [`App`](crate::App).
///
/// The builder consumes the live [`SidebarState`] + the current
/// [`View`] + the active [`Pane`] so (UNIFY-7):
///   * each group renders a header with a `+`/`–` collapse
///     chevron that reflects the reused [`SidebarState`] state,
///   * the active group is highlighted and auto-expanded,
///   * each item carries a 5 px status pip + the active item the
///     accent left-border + `raised` fill,
///   * a focus ring appears on the active row when the sidebar
///     pane holds keyboard focus (UX-5 (f)).
pub fn view<'a>(
    state: &'a SidebarState,
    view: View,
    focused_pane: Pane,
    on_group_click: impl Fn(Group) -> crate::Message + 'a,
    on_group_toggle: impl Fn(Group) -> crate::Message + 'a,
    on_panel_click: impl Fn(Group, &'static str) -> crate::Message + 'a,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let palette = crate::live_theme::palette();
    let active = view.group();
    let sidebar_focused = focused_pane == Pane::Sidebar;

    let mut col: Column<'a, crate::Message, cosmic::Theme> = column![].spacing(0);

    // UNIFY-7 — render the locked sections as a dense, divider-free accordion
    // (design lines 51-60): one header per group, its items shown only while the
    // group is expanded.
    for entry in nav_model() {
        let expanded = state.is_expanded(entry.group, active);
        col = col.push(group_header(
            entry.group,
            active,
            expanded,
            palette,
            &on_group_click,
            &on_group_toggle,
        ));
        if expanded {
            for panel in &entry.panels {
                let is_active = matches!(
                    view,
                    View::Panel { group, panel: slug }
                        if group == entry.group && slug == panel.slug()
                );
                col = col.push(nav_item(
                    entry.group,
                    panel.slug(),
                    panel.label(),
                    is_active,
                    sidebar_focused,
                    palette,
                    &on_panel_click,
                ));
            }
        }
    }

    container(col)
        .width(Length::Fixed(SIDEBAR_WIDTH))
        .height(Length::Fill)
        // UNIFY-7 (design `<nav>` line 51) — 6 px vertical breathing room; the
        // group headers + items own their horizontal padding.
        .padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 6.0,
            left: 0.0,
        })
        .sty(move |_theme| container::Style {
            icon_color: None,
            snap: false,
            // UNIFY-7 (design aside line 46) — the nav panel is `surface`
            // (#262626), one step lighter than the `background` content area.
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 0.0.into(),
            },
            shadow: Shadow::default(),
            text_color: Some(palette.text.into_cosmic_color()),
        })
        .into()
}

/// UNIFY-7 — collapsible group header (design line 53). An uppercase
/// 11 sp label on the left, a `+`/`–` collapse chevron on the right.
/// The chevron is `–` while the group is expanded and `+` while
/// collapsed, reflecting the reused [`SidebarState`] collapse. The
/// header click routes through the existing `on_group_click` callback
/// (navigates to the group, which auto-expands it as the active group);
/// a chevron that toggles collapse *independently* of navigation needs a
/// new callback wired in `app.rs` (the `ToggleGroupExpansion` message
/// already exists) and is left as a follow-up.
fn group_header<'a>(
    group: Group,
    active: Group,
    expanded: bool,
    palette: Palette,
    on_click: &(impl Fn(Group) -> crate::Message + 'a),
    on_toggle: &(impl Fn(Group) -> crate::Message + 'a),
) -> Element<'a, crate::Message, cosmic::Theme> {
    let is_active = group == active;
    // Design line 619 — active header in `text`, inactive in `text_muted`.
    let label_color = if is_active {
        palette.text.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    };

    let label = text(group.label().to_uppercase())
        .size(SECTION_LABEL_SIZE)
        .font(Font {
            weight: cosmic::iced::font::Weight::Medium,
            ..Font::DEFAULT
        })
        .colr(label_color)
        .align_y(alignment::Vertical::Center);

    // U+2013 EN DASH when open, "+" when collapsed (design line 618).
    let chevron = text(if expanded { "\u{2013}" } else { "+" })
        .size(CHEVRON_SIZE)
        .colr(palette.text_muted.into_cosmic_color())
        .align_y(alignment::Vertical::Center);

    // UNIFY-7 follow-up — split the header so the LABEL navigates (on_click) while
    // the CHEVRON toggles collapse *independently* (on_toggle →
    // Message::ToggleGroupExpansion), instead of the chevron being inert.
    let label_btn = button(row![label, Space::new().width(Length::Fill)])
        .width(Length::Fill)
        .padding(Padding {
            top: 6.0,
            right: 6.0,
            bottom: 6.0,
            left: 13.0,
        })
        .on_press(on_click(group))
        .sty(header_btn_style(palette, label_color));
    let chevron_btn = button(chevron)
        .padding(Padding {
            top: 6.0,
            right: 14.0,
            bottom: 6.0,
            left: 6.0,
        })
        .on_press(on_toggle(group))
        .sty(header_btn_style(
            palette,
            palette.text_muted.into_cosmic_color(),
        ));

    row![label_btn, chevron_btn]
        .align_y(alignment::Vertical::Center)
        .into()
}

/// Shared hover/press style for the split group-header buttons (UNIFY-7).
fn header_btn_style(
    palette: Palette,
    text_color: Color,
) -> impl Fn(&cosmic::Theme, ButtonStatus) -> button::Style {
    move |_theme, status| {
        let bg = match status {
            ButtonStatus::Hovered => Background::Color(palette.raised.into_cosmic_color()),
            ButtonStatus::Pressed => Background::Color(palette.overlay.into_cosmic_color()),
            _ => Background::Color(Color::TRANSPARENT),
        };
        let border = Border::default();
        button::Style {
            snap: false,
            icon_color: None,
            background: Some(bg),
            text_color,
            border,
            border_color: border.color,
            border_width: border.width,
            border_radius: border.radius,
            shadow: Shadow::default(),
        }
    }
}

/// UNIFY-7 — a single nav item inside an expanded group (design line 624).
/// Hosts:
///   * a 3 px accent left-border drawn full-height when `is_active`,
///   * a 5 px status pip (reuses `panel_chrome::status_dot_sized`),
///   * the curated label at 13 sp (medium weight when active),
///   * a `raised` surface fill when active,
///   * a focus ring when the sidebar pane is focused + this is the
///     active item (UX-5 (f)).
///
/// §7 status pip: every panel in [`nav_model`] routes to a real
/// `panel_body` view in `app.rs` (verified 2026-06-28; none fall to the
/// `panel_under_construction` catch-all), so the pip reads the `success`
/// ("ready") token for every item — we do NOT fabricate a "not-yet"
/// subset. If a panel is ever listed in the nav before its view ships,
/// its not-yet status would have to be threaded in from `app.rs` (the
/// sole owner of the routing), rendered with `text_muted`.
fn nav_item<'a>(
    group: Group,
    slug: &'static str,
    label_text: &'static str,
    is_active: bool,
    sidebar_focused: bool,
    palette: Palette,
    on_click: &(impl Fn(Group, &'static str) -> crate::Message + 'a),
) -> Element<'a, crate::Message, cosmic::Theme> {
    // Accent left-border (design `border-left:3px solid acc`), full row height.
    let stripe_color = if is_active {
        palette.accent.into_cosmic_color()
    } else {
        Color::TRANSPARENT
    };
    let stripe = container(Space::new().height(Length::Fixed(NAV_ROW_HEIGHT)))
        .width(Length::Fixed(SELECTED_STRIPE_WIDTH))
        .height(Length::Fixed(NAV_ROW_HEIGHT))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(stripe_color)),
            ..container::Style::default()
        });

    let dot = crate::panel_chrome::status_dot_sized(
        palette.success.into_cosmic_color(),
        NAV_DOT_DIAMETER,
    );

    let weight = if is_active {
        cosmic::iced::font::Weight::Medium
    } else {
        cosmic::iced::font::Weight::Normal
    };
    let label = text(label_text)
        .size(NAV_LABEL_SIZE)
        .font(Font {
            weight,
            ..Font::DEFAULT
        })
        .colr(palette.text.into_cosmic_color())
        .align_y(alignment::Vertical::Center);

    // Pip + label, indented to the design's 24 px (3 px stripe + 21 px pad);
    // `gap:8px` between pip and label, `4px` vertical (design line 624).
    let content = container(
        row![dot, label]
            .spacing(8.0)
            .align_y(alignment::Vertical::Center),
    )
    .padding(Padding {
        top: 4.0,
        right: 14.0,
        bottom: 4.0,
        left: 21.0,
    });

    let inner = row![stripe, content]
        .align_y(alignment::Vertical::Center)
        .height(Length::Fixed(NAV_ROW_HEIGHT));

    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
        // Design line 624 — active item filled with `raised` (#393939);
        // hover/press give inactive rows the same affordance idiom as before.
        let bg = if is_active {
            Background::Color(palette.raised.into_cosmic_color())
        } else {
            match status {
                ButtonStatus::Hovered => Background::Color(palette.raised.into_cosmic_color()),
                ButtonStatus::Pressed => Background::Color(palette.overlay.into_cosmic_color()),
                _ => Background::Color(Color::TRANSPARENT),
            }
        };
        let border = if is_active && sidebar_focused {
            Border {
                color: palette.accent.into_cosmic_color(),
                width: FOCUS_RING_WIDTH,
                radius: 0.0.into(),
            }
        } else {
            Border::default()
        };
        button::Style {
            snap: false,
            icon_color: None,
            background: Some(bg),
            text_color: palette.text.into_cosmic_color(),
            border,
            border_color: border.color,
            border_width: border.width,
            border_radius: border.radius,
            shadow: Shadow::default(),
        }
    };

    button(inner)
        .width(Length::Fill)
        .height(Length::Fixed(NAV_ROW_HEIGHT))
        .padding(0)
        .on_press(on_click(group, slug))
        .sty(style)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_no_user_expansions() {
        let state = SidebarState::new();
        assert!(!state.is_expanded(Group::Mesh, Group::Dashboard));
    }

    #[test]
    fn active_group_is_always_expanded() {
        let state = SidebarState::new();
        for active in Group::all() {
            assert!(
                state.is_expanded(active, active),
                "active group {active:?} should always be expanded"
            );
        }
    }

    #[test]
    fn toggle_expands_then_collapses_inactive_group() {
        let mut state = SidebarState::new();
        let active = Group::Dashboard;
        assert!(!state.is_expanded(Group::ThisNode, active));
        state.toggle(Group::ThisNode, active);
        assert!(state.is_expanded(Group::ThisNode, active));
        state.toggle(Group::ThisNode, active);
        assert!(!state.is_expanded(Group::ThisNode, active));
    }

    #[test]
    fn toggle_on_active_group_is_noop() {
        let mut state = SidebarState::new();
        state.toggle(Group::Dashboard, Group::Dashboard);
        // The internal storage should stay empty — the active
        // group is implicitly expanded, never explicitly.
        assert!(state.user_expanded.is_empty());
        assert!(state.is_expanded(Group::Dashboard, Group::Dashboard));
    }

    #[test]
    fn multiple_groups_can_be_user_expanded_simultaneously() {
        let mut state = SidebarState::new();
        let active = Group::Dashboard;
        state.toggle(Group::Mesh, active);
        state.toggle(Group::ThisNode, active);
        assert!(state.is_expanded(Group::Mesh, active));
        assert!(state.is_expanded(Group::ThisNode, active));
        assert!(!state.is_expanded(Group::Fleet, active));
    }

    // Component-dimension locks. These guard the nav spec from
    // silent drift; UNIFY-7 retuned them to the Unified Workbench
    // design import (`docs/design/workbench/Workbench.dc.html`).
    // The matching docs/WORKLIST.md UX-5 marker update is a
    // follow-up (out of this unit's file scope).

    #[test]
    fn sidebar_width_locked_to_ux5_spec() {
        // UX-5 (a) — 240 px fixed width (kept across UNIFY-7).
        assert!((SIDEBAR_WIDTH - 240.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_row_height_locked_to_unify7_spec() {
        // UNIFY-7 — 28 px dense nav row (was UX-5's 40 px).
        assert!((NAV_ROW_HEIGHT - 28.0).abs() < f32::EPSILON);
    }

    #[test]
    fn selected_stripe_locked_to_three_px() {
        // UNIFY-7 (design line 624) — 3 px accent left border on
        // the active item (was UX-5's 2 px stripe).
        assert!((SELECTED_STRIPE_WIDTH - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_ring_locked_to_two_px() {
        // UX-5 (f) — 2 px accent focus ring when keyboard
        // navigation is in the sidebar pane.
        assert!((FOCUS_RING_WIDTH - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn section_label_is_eleven_sp() {
        // UX-5 (e) / UNIFY-7 — 11 sp all-caps group header.
        assert!((SECTION_LABEL_SIZE - 11.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_label_is_thirteen_sp() {
        // UNIFY-7 (design line 624) — 13 sp nav item label.
        assert!((NAV_LABEL_SIZE - 13.0).abs() < f32::EPSILON);
    }

    #[test]
    fn chevron_size_locked_to_unify7_spec() {
        // UNIFY-7 (design line 53) — 14 sp `+`/`–` collapse chevron.
        assert!((CHEVRON_SIZE - 14.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_dot_diameter_locked_to_unify7_spec() {
        // UNIFY-7 (design line 625) — 5 px status pip.
        assert!((NAV_DOT_DIAMETER - 5.0).abs() < f32::EPSILON);
    }
}
