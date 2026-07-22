//! Shared **HIG navigation chrome** — the standard [`NavigationBar`],
//! [`Toolbar`], and [`Sidebar`] every E12 surface composes instead of
//! hand-rolling its own top bar, action row, and grouped selectable list
//! (§6 glue; the navigation sibling of [`crate::widgets`]).
//!
//! PLATFORM-INTERFACES Q19: navigation is SHARED components in `mde-egui` —
//! NavigationBar / Toolbar / Sidebar land here first and the 17 surfaces adopt
//! them, so the platform's wayfinding reads as ONE system (back affordance,
//! title placement, action slots, and the Settings/Files-style section list all
//! come from a single source instead of seventeen near-copies).
//!
//! Everything renders through the shared kit: [`Style`] tokens only (type ramp,
//! spacing grid, radius tiers, control heights), Carbon glyphs via
//! [`crate::carbon::paint_carbon`], hover/press micro-interactions through the
//! reduce-motion-aware [`Motion`] primitives, and the platform 2 px focus ring
//! via [`crate::focus::paint_focus_ring`]. Activation follows the egui
//! `Response` idiom — callers read `clicked`-style booleans / returned ids each
//! frame, never callbacks.

use std::hash::Hash;

use egui::{
    Align2, Color32, FontId, Key, Modifiers, Rect, Response, Sense, Ui, Vec2, WidgetInfo,
    WidgetType,
};

use crate::{
    carbon::paint_carbon,
    focus::paint_focus_ring,
    motion::Motion,
    style::{Density, Style},
};

// ── Chrome heights (Density-aware) ──────────────────────────────────────────
// Density scales the spacing family and the hit-target floor — never a drawn
// dimension (design lock #7 / UX-24) — so these heights start from the control
// ladder and only GROW when the density's hit target plus its scaled gutter no
// longer fits. No new magic numbers: every term is an existing Style token
// (the comfortable ~52 pt bar is CONTROL_H_L + SP_M on the 8 px grid).

/// The comfortable [`NavigationBar`] strip height in logical points —
/// [`Style::CONTROL_H_L`] + [`Style::SP_M`] (52) on the shared control ladder.
const NAV_STRIP_H: f32 = Style::CONTROL_H_L + Style::SP_M;

/// The [`Toolbar`] base height — a standard control row
/// ([`Style::CONTROL_H_M`]) plus one base gutter ([`Style::SP_S`]).
const TOOLBAR_BASE_H: f32 = Style::CONTROL_H_M + Style::SP_S;

/// [`NavigationBar`] height for `density` — the comfortable strip, grown only
/// when the density's hit target plus a scaled gutter no longer fits.
#[must_use]
pub fn nav_bar_height(density: Density) -> f32 {
    NAV_STRIP_H.max(density.min_hit_target() + Style::SP_S * density.spacing_scale())
}

/// [`NavigationBar`] height for the `large_title` scroll-top variant: the
/// standard strip plus a hero band tall enough for the
/// [`Style::TYPE_LARGE_TITLE`] rung and one base gutter.
#[must_use]
pub fn nav_bar_large_height(density: Density) -> f32 {
    nav_bar_height(density) + Style::TYPE_LARGE_TITLE + Style::SP_S
}

/// [`Toolbar`] height for `density` — a standard control row plus a gutter,
/// grown to fit the density's hit target under touch.
#[must_use]
pub fn toolbar_height(density: Density) -> f32 {
    TOOLBAR_BASE_H.max(density.min_hit_target() + Style::SP_XS * density.spacing_scale())
}

/// [`Sidebar`] row height for `density` — the large control rung
/// ([`Style::CONTROL_H_L`]), grown to the density's hit-target floor.
#[must_use]
pub fn sidebar_row_height(density: Density) -> f32 {
    Style::CONTROL_H_L.max(density.min_hit_target())
}

/// Shared activation test for the chrome buttons: a pointer click, or Enter
/// while the widget holds keyboard focus (consumed so no other reader re-fires
/// it) — every bar affordance is keyboard-activatable (WCAG 2.1.1).
fn activated(ui: &Ui, response: &Response) -> bool {
    response.clicked()
        || (response.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)))
}

/// The translucent hover wash for a chrome affordance: [`Style::SURFACE_HI`]
/// faded in by the reduce-motion-aware hover progress `t` (`0` = rest).
fn hover_wash(ui: &Ui, t: f32) -> Color32 {
    Style::resolve_color(ui.ctx(), Style::SURFACE_HI).gamma_multiply(Motion::hover_lift(t))
}

/// Paint one square icon affordance (hover wash, press squash, Carbon glyph,
/// focus ring) into `rect` and report the interaction — the shared body of a
/// [`NavigationBar`] action slot and an icon [`Toolbar`] item.
fn icon_button(
    ui: &Ui,
    rect: Rect,
    id: egui::Id,
    icon: &str,
    label: &str,
    tint: Color32,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let hover = Motion::animate(ui.ctx(), id.with("hover"), response.hovered(), Motion::FAST);
        if hover > 0.0 {
            ui.painter()
                .rect_filled(rect, Style::RADIUS_S, hover_wash(ui, hover));
        }
        let press = Motion::animate(
            ui.ctx(),
            id.with("press"),
            response.is_pointer_button_down_on(),
            Motion::FAST,
        );
        let side = Style::ICON_L * Motion::press_scale(press);
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(side));
        // An unknown glyph simply paints nothing — layout stays stable, matching
        // the carbon loader's speculative-call contract.
        let _ = paint_carbon(ui.painter(), icon_rect, icon, tint);
        paint_focus_ring(ui.painter(), rect, response.has_focus());
    }
    response
}

// ── NavigationBar ────────────────────────────────────────────────────────────

/// One trailing action slot on a [`NavigationBar`]: a Carbon glyph button with
/// an accessible label ([`crate::carbon_names`] lists the embedded glyphs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavAction<'a> {
    /// Mackes-Carbon glyph name (e.g. `"view-refresh"`).
    pub icon: &'a str,
    /// Accessible label announced for the slot (also the AccessKit name).
    pub label: &'a str,
}

impl<'a> NavAction<'a> {
    /// A trailing action slot: Carbon glyph `icon` announced as `label`.
    #[must_use]
    pub const fn new(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label }
    }
}

/// PLATFORM-INTERFACES Q19 — the standard in-app **top bar**: an optional back
/// affordance (chevron + previous title), a centered (or leading) title on the
/// [`Style::TYPE_TITLE3`] rung — or the [`Style::TYPE_LARGE_TITLE`] hero rung in
/// the scroll-top [`large_title`](Self::large_title) variant — and trailing
/// Carbon action slots. Show with [`show`](Self::show); read activation off the
/// returned [`NavBarResponse`].
#[derive(Debug, Clone, Copy)]
pub struct NavigationBar<'a> {
    title: &'a str,
    back: Option<&'a str>,
    large: bool,
    leading: bool,
    actions: &'a [NavAction<'a>],
}

impl<'a> NavigationBar<'a> {
    /// A bar titled `title` (centered, standard height, no back, no actions).
    #[must_use]
    pub const fn new(title: &'a str) -> Self {
        Self {
            title,
            back: None,
            large: false,
            leading: false,
            actions: &[],
        }
    }

    /// Add the back affordance: an accent chevron plus `previous` (the title of
    /// the surface the operator returns to), keyboard-activatable.
    #[must_use]
    pub const fn with_back(mut self, previous: &'a str) -> Self {
        self.back = Some(previous);
        self
    }

    /// The scroll-top state: the title renders leading on the
    /// [`Style::TYPE_LARGE_TITLE`] hero rung in a taller bar.
    #[must_use]
    pub const fn large_title(mut self) -> Self {
        self.large = true;
        self
    }

    /// Lead-align the standard title instead of centering it.
    #[must_use]
    pub const fn leading_title(mut self) -> Self {
        self.leading = true;
        self
    }

    /// Attach the trailing action slots (rendered right-to-left from the
    /// trailing edge, in slice order).
    #[must_use]
    pub const fn with_actions(mut self, actions: &'a [NavAction<'a>]) -> Self {
        self.actions = actions;
        self
    }

    /// Render the bar across the available width and report the interactions.
    pub fn show(self, ui: &mut Ui) -> NavBarResponse {
        let density = Style::density(ui.ctx());
        let strip_h = nav_bar_height(density);
        let bar_h = if self.large {
            nav_bar_large_height(density)
        } else {
            strip_h
        };
        let (rect, bar) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), bar_h), Sense::hover());
        let mut out = NavBarResponse {
            bar,
            back: None,
            back_activated: false,
            actions: Vec::new(),
            activated_action: None,
        };
        if !ui.is_rect_visible(rect) {
            return out;
        }

        // The quiet chrome ground: surface fill, hairline on the content edge.
        ui.painter()
            .rect_filled(rect, 0.0, Style::resolve_color(ui.ctx(), Style::SURFACE));
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - Style::STROKE_HAIRLINE * 0.5,
            Style::hairline(),
        );

        let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), strip_h));
        // Hit rects honour the density floor without outgrowing the strip.
        let hit = density
            .min_hit_target()
            .max(Style::CONTROL_H_S)
            .min(strip.height());
        let mut title_left = strip.left() + Style::SP_M;

        // Back affordance — accent chevron + previous title, one hit target.
        if let Some(previous) = self.back {
            let accent = Style::resolve_color(ui.ctx(), Style::ACCENT);
            let galley = ui.painter().layout_no_wrap(
                previous.to_owned(),
                FontId::proportional(Style::TYPE_BODY),
                accent,
            );
            let width = Style::SP_S + Style::ICON_L + Style::SP_XS + galley.size().x + Style::SP_S;
            let back_rect = Rect::from_min_size(
                egui::pos2(strip.left() + Style::SP_S, strip.center().y - hit * 0.5),
                Vec2::new(width, hit),
            );
            let id = ui.id().with("mde-nav-back");
            let response = ui.interact(back_rect, id, Sense::click());
            response.widget_info(|| {
                WidgetInfo::labeled(
                    WidgetType::Button,
                    ui.is_enabled(),
                    format!("Back to {previous}"),
                )
            });
            let hover =
                Motion::animate(ui.ctx(), id.with("hover"), response.hovered(), Motion::FAST);
            if hover > 0.0 {
                ui.painter()
                    .rect_filled(back_rect, Style::RADIUS_S, hover_wash(ui, hover));
            }
            let press = Motion::animate(
                ui.ctx(),
                id.with("press"),
                response.is_pointer_button_down_on(),
                Motion::FAST,
            );
            let side = Style::ICON_L * Motion::press_scale(press);
            let chevron = Rect::from_center_size(
                egui::pos2(
                    back_rect.left() + Style::SP_S + Style::ICON_L * 0.5,
                    back_rect.center().y,
                ),
                Vec2::splat(side),
            );
            let _ = paint_carbon(ui.painter(), chevron, "go-previous", accent);
            ui.painter().galley(
                egui::pos2(
                    back_rect.left() + Style::SP_S + Style::ICON_L + Style::SP_XS,
                    back_rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                accent,
            );
            paint_focus_ring(ui.painter(), back_rect, response.has_focus());
            out.back_activated = activated(ui, &response);
            title_left = back_rect.right() + Style::SP_M;
            out.back = Some(response);
        }

        // Trailing action slots, right-to-left from the trailing edge.
        let mut right = strip.right() - Style::SP_S;
        let tint = Style::resolve_color(ui.ctx(), Style::TEXT);
        for (index, action) in self.actions.iter().enumerate() {
            let slot = Rect::from_center_size(
                egui::pos2(right - hit * 0.5, strip.center().y),
                Vec2::splat(hit),
            );
            let id = ui.id().with(("mde-nav-action", index));
            let response = icon_button(ui, slot, id, action.icon, action.label, tint);
            if activated(ui, &response) {
                out.activated_action = Some(index);
            }
            right = slot.left() - Style::SP_XS;
            out.actions.push(response);
        }

        // Title — hero rung leading in the large band, else Title3 in the strip,
        // clipped to the free middle region so it never paints under an action.
        let strong = Style::resolve_color(ui.ctx(), Style::TEXT_STRONG);
        let clip = Rect::from_min_max(
            egui::pos2(title_left, rect.top()),
            egui::pos2(right - Style::SP_XS, rect.bottom()),
        );
        let painter = ui.painter().with_clip_rect(clip);
        if self.large {
            painter.text(
                egui::pos2(rect.left() + Style::SP_M, rect.bottom() - Style::SP_S),
                Align2::LEFT_BOTTOM,
                self.title,
                FontId::proportional(Style::TYPE_LARGE_TITLE),
                strong,
            );
        } else if self.leading {
            painter.text(
                egui::pos2(title_left, strip.center().y),
                Align2::LEFT_CENTER,
                self.title,
                FontId::proportional(Style::TYPE_TITLE3),
                strong,
            );
        } else {
            painter.text(
                strip.center(),
                Align2::CENTER_CENTER,
                self.title,
                FontId::proportional(Style::TYPE_TITLE3),
                strong,
            );
        }

        out
    }
}

/// What a [`NavigationBar`] reported this frame, in the egui `Response` idiom.
#[derive(Debug, Clone)]
pub struct NavBarResponse {
    /// The whole-bar response (hover sensing / context menus).
    pub bar: Response,
    /// The back affordance's response, when a back was configured.
    pub back: Option<Response>,
    /// `true` when back fired this frame — click, or Enter while focused.
    pub back_activated: bool,
    /// One response per trailing action, in [`NavigationBar::with_actions`] order.
    pub actions: Vec<Response>,
    /// The index of the trailing action that fired this frame, if any.
    pub activated_action: Option<usize>,
}

// ── Toolbar ──────────────────────────────────────────────────────────────────

/// One [`Toolbar`] action: a labeled text action, or a Carbon glyph action
/// whose `label` is announced (AccessKit) but not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarItem<'a> {
    /// Carbon glyph name when this is an icon action; `None` draws the label.
    pub icon: Option<&'a str>,
    /// The drawn text (labeled form) or the accessible name (icon form).
    pub label: &'a str,
}

impl<'a> ToolbarItem<'a> {
    /// A text action — `label` is drawn.
    #[must_use]
    pub const fn labeled(label: &'a str) -> Self {
        Self { icon: None, label }
    }

    /// A glyph action — Carbon `icon` is drawn, `label` is the accessible name.
    #[must_use]
    pub const fn icon(icon: &'a str, label: &'a str) -> Self {
        Self {
            icon: Some(icon),
            label,
        }
    }
}

/// PLATFORM-INTERFACES Q19 — the standard **action row**: leading and trailing
/// groups of labeled or Carbon-glyph actions on a quiet surface strip,
/// separator-free, Density-aware height. Defaults to the bottom-of-surface
/// placement (hairline on its top edge); call [`at_top`](Self::at_top) when the
/// row sits above its content instead.
#[derive(Debug, Clone, Copy, Default)]
pub struct Toolbar<'a> {
    leading: &'a [ToolbarItem<'a>],
    trailing: &'a [ToolbarItem<'a>],
    at_top: bool,
}

impl<'a> Toolbar<'a> {
    /// An empty toolbar (bottom placement).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            leading: &[],
            trailing: &[],
            at_top: false,
        }
    }

    /// The leading action group, laid out from the leading edge in order.
    #[must_use]
    pub const fn leading(mut self, items: &'a [ToolbarItem<'a>]) -> Self {
        self.leading = items;
        self
    }

    /// The trailing action group, hugging the trailing edge in order.
    #[must_use]
    pub const fn trailing(mut self, items: &'a [ToolbarItem<'a>]) -> Self {
        self.trailing = items;
        self
    }

    /// Place the row above its content: the hairline moves to the bottom edge.
    #[must_use]
    pub const fn at_top(mut self) -> Self {
        self.at_top = true;
        self
    }

    /// Render the row across the available width and report the interactions.
    /// Item indices run leading group first, then trailing, in slice order.
    pub fn show(self, ui: &mut Ui) -> ToolbarResponse {
        let density = Style::density(ui.ctx());
        let sp = density.spacing_scale();
        let height = toolbar_height(density);
        let (rect, bar) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let mut out = ToolbarResponse {
            bar,
            items: Vec::new(),
            activated: None,
        };
        if !ui.is_rect_visible(rect) {
            return out;
        }

        ui.painter()
            .rect_filled(rect, 0.0, Style::resolve_color(ui.ctx(), Style::SURFACE));
        let edge_y = if self.at_top {
            rect.bottom() - Style::STROKE_HAIRLINE * 0.5
        } else {
            rect.top() + Style::STROKE_HAIRLINE * 0.5
        };
        ui.painter()
            .hline(rect.x_range(), edge_y, Style::hairline());

        let hit = density.min_hit_target().min(rect.height());
        let accent = Style::resolve_color(ui.ctx(), Style::ACCENT);
        let gap = Style::SP_XS * sp;

        // Leading group, left-to-right from the leading edge.
        let mut x = rect.left() + Style::SP_S * sp;
        let mut index = 0usize;
        for item in self.leading {
            let width = self.item_width(ui, item, hit, sp);
            let slot = Rect::from_min_size(
                egui::pos2(x, rect.center().y - hit * 0.5),
                Vec2::new(width, hit),
            );
            self.show_item(ui, slot, index, item, accent, &mut out);
            x = slot.right() + gap;
            index += 1;
        }

        // Trailing group: measure first so the group hugs the trailing edge
        // while responses stay in slice (doc) order.
        let widths: Vec<f32> = self
            .trailing
            .iter()
            .map(|item| self.item_width(ui, item, hit, sp))
            .collect();
        let group: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
        let mut x = rect.right() - Style::SP_S * sp - group;
        for (item, width) in self.trailing.iter().zip(widths) {
            let slot = Rect::from_min_size(
                egui::pos2(x, rect.center().y - hit * 0.5),
                Vec2::new(width, hit),
            );
            self.show_item(ui, slot, index, item, accent, &mut out);
            x = slot.right() + gap;
            index += 1;
        }

        out
    }

    /// The slot width for `item`: square for a glyph, measured text plus the
    /// scaled padding for a label.
    fn item_width(self, ui: &Ui, item: &ToolbarItem<'_>, hit: f32, sp: f32) -> f32 {
        if item.icon.is_some() {
            hit
        } else {
            let galley = ui.painter().layout_no_wrap(
                item.label.to_owned(),
                FontId::proportional(Style::MENU_TEXT),
                Color32::PLACEHOLDER,
            );
            galley.size().x + 2.0 * Style::SP_S * sp
        }
    }

    /// Render one action slot into `slot` and fold its interaction into `out`.
    fn show_item(
        self,
        ui: &Ui,
        slot: Rect,
        index: usize,
        item: &ToolbarItem<'_>,
        accent: Color32,
        out: &mut ToolbarResponse,
    ) {
        let id = ui.id().with(("mde-nav-toolbar", index));
        let response = if let Some(icon) = item.icon {
            icon_button(ui, slot, id, icon, item.label, accent)
        } else {
            let response = ui.interact(slot, id, Sense::click());
            response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), item.label)
            });
            if ui.is_rect_visible(slot) {
                let hover =
                    Motion::animate(ui.ctx(), id.with("hover"), response.hovered(), Motion::FAST);
                if hover > 0.0 {
                    ui.painter()
                        .rect_filled(slot, Style::RADIUS_S, hover_wash(ui, hover));
                }
                ui.painter().text(
                    slot.center(),
                    Align2::CENTER_CENTER,
                    item.label,
                    FontId::proportional(Style::MENU_TEXT),
                    accent,
                );
                paint_focus_ring(ui.painter(), slot, response.has_focus());
            }
            response
        };
        if activated(ui, &response) {
            out.activated = Some(index);
        }
        out.items.push(response);
    }
}

/// What a [`Toolbar`] reported this frame.
#[derive(Debug, Clone)]
pub struct ToolbarResponse {
    /// The whole-row response.
    pub bar: Response,
    /// One response per action — leading group first, then trailing.
    pub items: Vec<Response>,
    /// The index (into [`items`](Self::items)) of the action that fired.
    pub activated: Option<usize>,
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

/// One selectable [`Sidebar`] row: a caller id, a label, an optional Carbon
/// glyph. Generic over the caller's id vocabulary (§6 — the surface keeps its
/// own action enum / key type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarRow<'a, Id> {
    /// The caller-supplied selection id this row reports.
    pub id: Id,
    /// The drawn row label (also the AccessKit name).
    pub label: &'a str,
    /// Optional Mackes-Carbon glyph drawn before the label.
    pub icon: Option<&'a str>,
}

impl<'a, Id> SidebarRow<'a, Id> {
    /// A glyph-less row.
    #[must_use]
    pub const fn new(id: Id, label: &'a str) -> Self {
        Self {
            id,
            label,
            icon: None,
        }
    }

    /// Add a Carbon glyph before the label.
    #[must_use]
    pub const fn with_icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// One [`Sidebar`] section: an optional dim [`Style::TYPE_FOOTNOTE`] header
/// over its rows.
#[derive(Debug, Clone, Copy)]
pub struct SidebarSection<'a, Id> {
    /// The dim section header, or `None` for an unlabeled group.
    pub header: Option<&'a str>,
    /// The section's rows, in draw order.
    pub rows: &'a [SidebarRow<'a, Id>],
}

/// PLATFORM-INTERFACES Q19 — the Settings/Files-style **grouped selectable
/// list**: dim footnote section headers, rows with an optional Carbon glyph and
/// an accent selection plate, arrow-key + Enter keyboard navigation. Stateless:
/// the caller owns the selected id and [`show`](Self::show) reports the new one.
pub struct Sidebar;

impl Sidebar {
    /// The deterministic egui id of row `index` (in top-to-bottom order across
    /// all sections) for the sidebar salted `id_salt`. Public so a caller (or a
    /// test) can hand a row keyboard focus programmatically; two sidebars in one
    /// app therefore need distinct salts.
    #[must_use]
    pub fn row_id(id_salt: impl Hash, index: usize) -> egui::Id {
        egui::Id::new("mde-egui-nav-sidebar")
            .with(id_salt)
            .with(index)
    }

    /// Render the grouped list and report the **newly selected id**, if any:
    /// a row click, an arrow-key move (Up/Down walk the flattened rows while a
    /// row holds keyboard focus — the list-box idiom, keys consumed), or Enter
    /// on the focused row. `None` means the selection did not change hands this
    /// frame. The caller passes its current `selected` id and stores the result.
    pub fn show<Id: Clone + PartialEq>(
        ui: &mut Ui,
        id_salt: impl Hash + Copy,
        sections: &[SidebarSection<'_, Id>],
        selected: &Id,
    ) -> Option<Id> {
        let density = Style::density(ui.ctx());
        let sp = density.spacing_scale();
        let row_h = sidebar_row_height(density);
        let flat: Vec<&SidebarRow<'_, Id>> = sections.iter().flat_map(|s| s.rows.iter()).collect();
        let total = flat.len();

        // Keyboard: while a row holds focus, arrows MOVE THE SELECTION and Enter
        // reports the focused row — consumed up front so this frame already
        // renders the new state (the finder/front-door idiom). The keys are
        // consumed for OTHER same-frame readers (nothing else scrolls), but
        // egui's own arrow focus traversal latched them in `begin_pass` and will
        // still walk the focus to the spatially adjacent row this frame — the
        // rows are equal-width stacked rects, so that lands exactly where the
        // selection moved. Deliberately no `request_focus` here: driving focus a
        // second time is what desynchronises the two (the traversal then steps
        // once more from the newly-focused row).
        let focused_row = ui
            .ctx()
            .memory(|m| m.focused())
            .and_then(|focus| (0..total).find(|&i| Self::row_id(id_salt, i) == focus));
        let mut out: Option<Id> = None;
        if let Some(row) = focused_row {
            let (up, down, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            let moved = if down && row + 1 < total {
                Some(row + 1)
            } else if up && row > 0 {
                Some(row - 1)
            } else {
                None
            };
            if let Some(next) = moved {
                out = Some(flat[next].id.clone());
            } else if enter {
                out = Some(flat[row].id.clone());
            }
        }

        // Paint against the selection as of this frame's keyboard handling; a
        // click below updates `out` and settles visually next frame (standard
        // immediate-mode one-frame catch-up).
        let effective: Id = out.clone().unwrap_or_else(|| selected.clone());
        let mut index = 0usize;
        for section in sections {
            if let Some(header) = section.header {
                let head_h = Style::TYPE_FOOTNOTE + Style::SP_S * sp;
                let (head, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), head_h), Sense::hover());
                if ui.is_rect_visible(head) {
                    ui.painter().text(
                        egui::pos2(head.left() + Style::SP_S, head.bottom() - Style::SP_XS * sp),
                        Align2::LEFT_BOTTOM,
                        header,
                        FontId::proportional(Style::TYPE_FOOTNOTE),
                        Style::resolve_color(ui.ctx(), Style::TEXT_DIM),
                    );
                }
            }
            for row in section.rows {
                Self::show_row(ui, id_salt, index, row, &effective, row_h, &mut out);
                index += 1;
            }
            ui.add_space(Style::SP_S * sp);
        }
        out
    }

    /// Render one selectable row and fold its interaction into `out`.
    fn show_row<Id: Clone + PartialEq>(
        ui: &mut Ui,
        id_salt: impl Hash,
        index: usize,
        row: &SidebarRow<'_, Id>,
        effective: &Id,
        row_h: f32,
        out: &mut Option<Id>,
    ) {
        let rid = Self::row_id(id_salt, index);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
        let response = ui.interact(rect, rid, Sense::click());
        let is_selected = row.id == *effective;
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::SelectableLabel,
                ui.is_enabled(),
                is_selected,
                row.label,
            )
        });
        if ui.is_rect_visible(rect) {
            let plate = rect.shrink2(Vec2::new(Style::SP_XS, Style::STROKE_HAIRLINE));
            if is_selected {
                // The shared selected-row accent plate (the selection idiom).
                ui.painter()
                    .rect_filled(plate, Style::RADIUS_S, Style::selection_fill());
            } else {
                let hover = Motion::animate(
                    ui.ctx(),
                    rid.with("hover"),
                    response.hovered(),
                    Motion::FAST,
                );
                if hover > 0.0 {
                    ui.painter()
                        .rect_filled(plate, Style::RADIUS_S, hover_wash(ui, hover));
                }
            }
            let mut text_x = plate.left() + Style::SP_S;
            if let Some(icon) = row.icon {
                let glyph = Rect::from_center_size(
                    egui::pos2(text_x + Style::ICON_M * 0.5, plate.center().y),
                    Vec2::splat(Style::ICON_M),
                );
                let tint = Style::resolve_color(
                    ui.ctx(),
                    if is_selected {
                        Style::TEXT
                    } else {
                        Style::TEXT_DIM
                    },
                );
                let _ = paint_carbon(ui.painter(), glyph, icon, tint);
                text_x = glyph.right() + Style::SP_S;
            }
            let color = Style::resolve_color(
                ui.ctx(),
                if is_selected {
                    Style::TEXT_STRONG
                } else {
                    Style::TEXT
                },
            );
            ui.painter().text(
                egui::pos2(text_x, plate.center().y),
                Align2::LEFT_CENTER,
                row.label,
                FontId::proportional(Style::TYPE_BODY),
                color,
            );
            paint_focus_ring(ui.painter(), plate, response.has_focus());
        }
        if response.clicked() {
            *out = Some(row.id.clone());
            ui.ctx().memory_mut(|m| m.request_focus(rid));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // tests fail by panicking, with context
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput};

    /// Collect every painted text run (string + font size) from a frame.
    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, f32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let size = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map_or(0.0, |s| s.format.font_id.size);
                    out.push((text.galley.text().to_owned(), size));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn key_press(key: Key) -> RawInput {
        RawInput {
            events: vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        }
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> RawInput {
        RawInput {
            events: vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn nav_bar_renders_title_and_back_fires_on_click_and_enter() {
        let ctx = egui::Context::default();
        let actions = [NavAction::new("view-refresh", "Refresh")];
        let mut last: Option<NavBarResponse> = None;
        let run = |input: RawInput, last: &mut Option<NavBarResponse>| {
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *last = Some(
                        NavigationBar::new("Settings")
                            .with_back("Home")
                            .with_actions(&actions)
                            .show(ui),
                    );
                });
            })
        };

        // Frame 1: the bar tessellates real chrome — title + previous title.
        let out = run(RawInput::default(), &mut last);
        assert!(!out.shapes.is_empty(), "nav bar must paint visible shapes");
        let texts = painted_text(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(t, s)| t == "Settings" && (*s - Style::TYPE_TITLE3).abs() < f32::EPSILON),
            "title must render on the Title3 rung: {texts:?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t == "Home"),
            "back affordance must carry the previous title: {texts:?}"
        );

        // Click path: press then release on the back affordance.
        let back = last.clone().unwrap().back.unwrap();
        let target = back.rect.center();
        let _ = run(pointer(target, true), &mut last);
        assert!(
            !last.as_ref().unwrap().back_activated,
            "press alone must not fire back"
        );
        let _ = run(pointer(target, false), &mut last);
        assert!(
            last.as_ref().unwrap().back_activated,
            "click release must fire back"
        );

        // Keyboard path: Enter while the back affordance holds focus.
        ctx.memory_mut(|m| m.request_focus(back.id));
        let _ = run(key_press(Key::Enter), &mut last);
        assert!(
            last.as_ref().unwrap().back_activated,
            "Enter on the focused back affordance must fire it"
        );
    }

    #[test]
    fn nav_bar_large_title_paints_the_hero_rung() {
        let ctx = egui::Context::default();
        let out = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = NavigationBar::new("Library").large_title().show(ui);
                let density = Style::density(ui.ctx());
                assert!(
                    (response.bar.rect.height() - nav_bar_large_height(density)).abs() < 0.5,
                    "large-title bar must use the taller band"
                );
            });
        });
        let texts = painted_text(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(t, s)| t == "Library"
                    && (*s - Style::TYPE_LARGE_TITLE).abs() < f32::EPSILON),
            "large title must render on the LARGE_TITLE rung: {texts:?}"
        );
    }

    #[test]
    fn toolbar_action_fires_on_click_and_reports_its_index() {
        let ctx = egui::Context::default();
        let leading = [ToolbarItem::labeled("Select")];
        let trailing = [
            ToolbarItem::icon("download", "Download"),
            ToolbarItem::labeled("Share"),
        ];
        let mut last: Option<ToolbarResponse> = None;
        let run = |input: RawInput, last: &mut Option<ToolbarResponse>| {
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *last = Some(
                        Toolbar::new()
                            .leading(&leading)
                            .trailing(&trailing)
                            .show(ui),
                    );
                });
            })
        };

        let out = run(RawInput::default(), &mut last);
        assert!(!out.shapes.is_empty(), "toolbar must paint visible shapes");
        let first = last.clone().unwrap();
        assert_eq!(first.items.len(), 3, "leading + trailing responses");
        assert!(first.activated.is_none());

        // Click the trailing glyph action (combined index 1: leading first).
        let target = first.items[1].rect.center();
        let _ = run(pointer(target, true), &mut last);
        let _ = run(pointer(target, false), &mut last);
        assert_eq!(
            last.as_ref().unwrap().activated,
            Some(1),
            "the clicked trailing action must report its combined index"
        );
    }

    #[test]
    fn sidebar_selection_moves_with_arrows_and_enter_reports_the_id() {
        let ctx = egui::Context::default();
        let rows = [
            SidebarRow::new("general", "General").with_icon("view-grid"),
            SidebarRow::new("network", "Network"),
            SidebarRow::new("display", "Display"),
        ];
        let sections = [SidebarSection {
            header: Some("Settings"),
            rows: &rows,
        }];
        let mut picked: Option<&str> = None;
        let run = |input: RawInput, picked: &mut Option<&str>| {
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *picked = Sidebar::show(ui, "settings-nav", &sections, &"general");
                });
            })
        };

        // Frame 1: headers + rows render; nothing picked.
        let out = run(RawInput::default(), &mut picked);
        assert!(!out.shapes.is_empty(), "sidebar must paint visible shapes");
        let texts = painted_text(&out.shapes);
        for expected in ["Settings", "General", "Network", "Display"] {
            assert!(
                texts.iter().any(|(t, _)| t == expected),
                "sidebar must render {expected:?}: {texts:?}"
            );
        }
        assert!(
            texts
                .iter()
                .any(|(t, s)| t == "Settings" && (*s - Style::TYPE_FOOTNOTE).abs() < f32::EPSILON),
            "section header must sit on the footnote rung: {texts:?}"
        );
        assert_eq!(picked, None);

        // Focus the first row, then walk the list with the arrows.
        ctx.memory_mut(|m| m.request_focus(Sidebar::row_id("settings-nav", 0)));
        let _ = run(key_press(Key::ArrowDown), &mut picked);
        assert_eq!(
            picked,
            Some("network"),
            "ArrowDown must select the next row"
        );
        let _ = run(key_press(Key::ArrowDown), &mut picked);
        assert_eq!(picked, Some("display"), "ArrowDown walks on down the list");
        let _ = run(key_press(Key::ArrowUp), &mut picked);
        assert_eq!(
            picked,
            Some("network"),
            "ArrowUp must select the previous row"
        );
        let _ = run(key_press(Key::Enter), &mut picked);
        assert_eq!(
            picked,
            Some("network"),
            "Enter must report the focused row's id"
        );
    }

    #[test]
    fn chrome_heights_respect_the_density_ladder() {
        // The pointer rungs keep the comfortable drawn heights; touch grows each
        // strip only as far as its hit-target floor demands (design lock #7 —
        // density never scales a drawn dimension directly).
        assert!((nav_bar_height(Density::Mouse) - NAV_STRIP_H).abs() < f32::EPSILON);
        assert!(nav_bar_height(Density::Touch) >= Density::Touch.min_hit_target() + Style::SP_S);
        assert!(
            nav_bar_large_height(Density::Mouse) > nav_bar_height(Density::Mouse),
            "the large-title band adds real height"
        );
        assert!(toolbar_height(Density::Mouse) >= Style::CONTROL_H_M);
        assert!(toolbar_height(Density::Touch) >= Density::Touch.min_hit_target());
        assert!(sidebar_row_height(Density::Mouse) >= Style::CONTROL_H_L);
        assert!(
            sidebar_row_height(Density::Touch) >= Density::Touch.min_hit_target(),
            "touch rows must reach the finger target"
        );
    }
}
