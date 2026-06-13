//! Iced sidebar widget.
//!
//! CB-1.2 lock: collapsible per-group rows. Pure-data
//! [`SidebarState`] (which groups are expanded, which row is
//! focused for keyboard navigation) lives here so the reducer
//! tests can stay Iced-free; the actual `view()` builder pulls
//! in Iced widgets.
//!
//! **UX-5 polish (2026-05-21)** — 240 px fixed width, every
//! colour / spacing / typography value drawn from `mde-theme`
//! tokens, selection / hover / focus states, section dividers,
//! and a reserved 20 px icon slot for the UX-8 glyph swap-in.
//! Component dimensions (sidebar width, nav row height, stripe
//! width) are NOT density-scaled — UX-24 sub-lock requires
//! density to scale spacing tokens only.

use cosmic::iced::widget::button::Status as ButtonStatus;
use cosmic::iced::widget::{button, column, container, row, text, Column, Space};
use cosmic::iced::{alignment, Background, Border, Color, Element, Length, Padding, Shadow};

use mde_theme::{Palette, Space as MdeSpace};

use crate::cosmic_compat::prelude::*;
use crate::keyboard::Pane;
use crate::model::{nav_model, Group, View};

/// UX-5 (a) — 240 px fixed sidebar width. Component dimension,
/// not density-scaled.
pub const SIDEBAR_WIDTH: f32 = 240.0;

/// UX-5 (b) — nav row + section-label component height.
pub const NAV_ROW_HEIGHT: f32 = 40.0;

/// UX-5 (b) — icon slot reserved for UX-8's `mde_icon()` swap-in.
/// Empty `Space` widget today; the layout already accounts for it
/// so post-UX-8 call sites change in one place per panel.
pub const NAV_ICON_SIZE: f32 = 20.0;

/// UX-5 (c) — accent stripe on the selected row's left edge.
const SELECTED_STRIPE_WIDTH: f32 = 2.0;

/// UX-5 (f) — focus-ring border width on the active row when the
/// sidebar pane holds keyboard focus.
const FOCUS_RING_WIDTH: f32 = 2.0;

/// UX-5 (b) — nav-row label point size. Locked at 14 sp (not
/// density-scaled per UX-24).
const NAV_LABEL_SIZE: f32 = 14.0;

/// UX-5 (e) — section-divider label point size.
const SECTION_LABEL_SIZE: f32 = 11.0;

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
/// [`View`] + the active [`Pane`] so:
///   * the active group is highlighted and auto-expanded,
///   * the active panel row carries the accent stripe + tint,
///   * a focus ring appears on the active row when the sidebar
///     pane holds keyboard focus (UX-5 (f)).
pub fn view<'a>(
    state: &'a SidebarState,
    view: View,
    focused_pane: Pane,
    on_group_click: impl Fn(Group) -> crate::Message + 'a,
    on_panel_click: impl Fn(Group, &'static str) -> crate::Message + 'a,
) -> Element<'a, crate::Message, cosmic::Theme> {
    let palette = crate::live_theme::palette();
    let space = MdeSpace::for_density(crate::live_theme::tokens().density);
    let active = view.group();
    let sidebar_focused = focused_pane == Pane::Sidebar;

    // UX-5 (a) — SPACE_16 ≈ md2 (17 px). Outer container padding.
    let outer_padding = f32::from(space.md2);

    let mut col: Column<'a, crate::Message, cosmic::Theme> = column![].spacing(0);

    // PLANES-1 (W4/W16) — the full five-plane tree is shown day-one:
    // Peers Front Door, the five planes (Network is a first-class plane
    // again, superseding the E4.15 hide), then the Desktop cluster.
    for (i, entry) in nav_model().into_iter().enumerate() {
        if i > 0 {
            col = col.push(section_divider(palette));
        }
        col = col.push(section_label(entry.group, active, palette, &on_group_click));
        if state.is_expanded(entry.group, active) {
            for panel in &entry.panels {
                let is_active = matches!(
                    view,
                    View::Panel { group, panel: slug }
                        if group == entry.group && slug == panel.slug()
                );
                col = col.push(nav_row(
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
        .padding(Padding {
            top: outer_padding,
            right: outer_padding,
            bottom: outer_padding,
            left: outer_padding,
        })
        .sty(move |_theme| container::Style {
            icon_color: None,
            snap: false,
            background: Some(Background::Color(palette.background.into_cosmic_color())),
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

/// UX-5 (e) — section divider. 1 px rule using the adaptive
/// border token.
fn section_divider<'a>(palette: Palette) -> Element<'a, crate::Message, cosmic::Theme> {
    container(cosmic::iced::widget::rule::horizontal(1))
        .padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 4.0,
            left: 0.0,
        })
        .sty(move |_| container::Style {
            snap: false,
            text_color: Some(palette.border.into_cosmic_color()),
            ..container::Style::default()
        })
        .into()
}

/// UX-5 (e) — section label above a group's panels. All-caps
/// 11 sp muted label, clickable to toggle the group's expansion
/// (preserves the CB-1.2 collapse contract; the label itself is
/// the section divider's title).
fn section_label<'a>(
    group: Group,
    active: Group,
    palette: Palette,
    on_click: &(impl Fn(Group) -> crate::Message + 'a),
) -> Element<'a, crate::Message, cosmic::Theme> {
    let is_active = group == active;
    let label_text = group.label().to_uppercase();
    let text_color = if is_active {
        palette.text.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    };

    let label = text(label_text)
        .size(SECTION_LABEL_SIZE)
        .colr(text_color)
        .align_y(alignment::Vertical::Center);

    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
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
    };

    button(label)
        .width(Length::Fill)
        .padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 4.0,
            left: 0.0,
        })
        .on_press(on_click(group))
        .sty(style)
        .into()
}

/// UX-5 — single nav row inside an expanded group. Hosts:
///   * 2 px accent stripe on the left when `is_active` (UX-5 c)
///   * reserved 20 px icon slot (UX-8 swap-in target, UX-5 b)
///   * label at 14 sp (UX-5 b)
///   * accent-tinted background when active (UX-5 c)
///   * surface-2 background on hover (UX-5 d)
///   * focus ring when the sidebar pane is focused + this is the
///     active row (UX-5 f)
fn nav_row<'a>(
    group: Group,
    slug: &'static str,
    label_text: &'static str,
    is_active: bool,
    sidebar_focused: bool,
    palette: Palette,
    on_click: &(impl Fn(Group, &'static str) -> crate::Message + 'a),
) -> Element<'a, crate::Message, cosmic::Theme> {
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

    let icon_slot = Space::new().width(Length::Fixed(NAV_ICON_SIZE));

    let text_color = if is_active {
        palette.accent.into_cosmic_color()
    } else {
        palette.text.into_cosmic_color()
    };
    let label = text(label_text)
        .size(NAV_LABEL_SIZE)
        .colr(text_color)
        .align_y(alignment::Vertical::Center);

    let content = row![
        stripe,
        Space::new().width(Length::Fixed(8.0)),
        icon_slot,
        Space::new().width(Length::Fixed(8.0)),
        label,
    ]
    .align_y(alignment::Vertical::Center)
    .height(Length::Fixed(NAV_ROW_HEIGHT));

    let style = move |_theme: &cosmic::Theme, status: ButtonStatus| {
        let bg = if is_active {
            Background::Color(palette.hover_tint().into_cosmic_color())
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
            text_color,
            border,
            border_color: border.color,
            border_width: border.width,
            border_radius: border.radius,
            shadow: Shadow::default(),
        }
    };

    button(content)
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
        assert!(!state.is_expanded(Group::Apps, Group::Dashboard));
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
        assert!(!state.is_expanded(Group::Network, active));
        state.toggle(Group::Network, active);
        assert!(state.is_expanded(Group::Network, active));
        state.toggle(Group::Network, active);
        assert!(!state.is_expanded(Group::Network, active));
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
        state.toggle(Group::Apps, active);
        state.toggle(Group::Network, active);
        assert!(state.is_expanded(Group::Apps, active));
        assert!(state.is_expanded(Group::Network, active));
        assert!(!state.is_expanded(Group::Fleet, active));
    }

    // UX-5 — component-dimension locks. These guard the
    // worklist UX-5 spec from silent drift; bumping any of
    // them requires a worklist edit + a design-doc note.

    #[test]
    fn sidebar_width_locked_to_ux5_spec() {
        // UX-5 (a) — 240 px fixed width.
        assert!((SIDEBAR_WIDTH - 240.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_row_height_locked_to_ux5_spec() {
        // UX-5 (b) — 40 px nav row.
        assert!((NAV_ROW_HEIGHT - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn icon_slot_width_locked_to_ux5_spec() {
        // UX-5 (b) — 20 px icon slot reserved for UX-8.
        assert!((NAV_ICON_SIZE - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn selected_stripe_locked_to_two_px() {
        // UX-5 (c) — 2 px accent left border on selected row.
        assert!((SELECTED_STRIPE_WIDTH - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_ring_locked_to_two_px() {
        // UX-5 (f) — 2 px accent focus ring when keyboard
        // navigation is in the sidebar pane.
        assert!((FOCUS_RING_WIDTH - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn section_label_is_eleven_sp() {
        // UX-5 (e) — 11 sp all-caps muted label.
        assert!((SECTION_LABEL_SIZE - 11.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_label_is_fourteen_sp() {
        // UX-5 (b) — 14 sp nav row label.
        assert!((NAV_LABEL_SIZE - 14.0).abs() < f32::EPSILON);
    }
}
