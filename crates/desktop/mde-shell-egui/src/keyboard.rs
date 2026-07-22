//! `keyboard` — SURFACE-10: the native egui **on-screen keyboard** (OSK), a shell
//! overlay (design `docs/design/surface-tablet-enablement.md`, lock 14).
//!
//! A convertible with the Type Cover detached has no physical keyboard, so the shell
//! must draw one and feed the focused text field itself (the bare-DRM seat owns the
//! input path — there is no external input-method to lean on). This module is that
//! OSK, built entirely on the Construct [`Style`]/[`Motion`] tokens (§4 — no raw hex).
//!
//! **Three parts, the first two pure + headless-testable:**
//!
//! 1. **The raise/dismiss fold** ([`resolve_visible`]) — the OSK is up exactly when
//!    the device is a [`Formfactor::Tablet`] **and** a text field wants keyboard input
//!    ([`egui::Context::wants_keyboard_input`]), unless the operator has manually
//!    forced it ([`Manual`]). It auto-dismisses the instant focus leaves or the cover
//!    re-attaches (→ Laptop). SURFACE-9's [`Formfactor`] flip drives the auto-engage.
//! 2. **The keypress → egui-event mapping** ([`cap_events`]) — a key emits the *same*
//!    [`egui::Event::Text`] / [`egui::Event::Key`] a hardware key would, so the focused
//!    field receives it identically. Injected events are queued and flushed into the
//!    next frame's input ([`Keyboard::flush_pending`]) so the field consumes them.
//! 3. **The layout state machine** ([`Layout`]) — QWERTY / numeric / symbols with a
//!    layer-toggle key and a one-shot [`Shift`](Cap::Shift).
//!
//! The overlay itself renders through an [`egui::Area`] in [`egui::Order::Foreground`]
//! (the same idiom as the KIRON chyron), floating above whatever surface is in view.
//! On the windowed dev fallback there is no seat, so the formfactor stays Laptop and
//! the whole OSK self-gates to honest silence (§7) — the pure folds are what the tests
//! exercise.

use mde_egui::egui::{self, RichText};
use mde_egui::{Formfactor, Motion, Style};
use mde_theme::brand::icons::IconId;

use crate::dock::icon_texture;

/// Which layer the OSK is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Layout {
    /// The letter layer (a-z) — the default.
    #[default]
    Qwerty,
    /// The digits + common-punctuation layer.
    Numeric,
    /// The extended-symbols layer.
    Symbols,
}

/// The operator's manual override of the auto raise/dismiss fold.
///
/// [`Auto`](Manual::Auto) follows focus; the tablet-bar toggle / dismiss control set
/// [`ForceShow`](Manual::ForceShow) / [`ForceHide`](Manual::ForceHide). An override is
/// transient: it decays back to `Auto` when focus leaves or the device returns to
/// Laptop, so the next focus auto-raises again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Manual {
    /// Follow the focus fold (the resting state).
    #[default]
    Auto,
    /// The operator raised the OSK by hand (holds even without a focused field).
    ForceShow,
    /// The operator dismissed the OSK by hand (holds until focus leaves).
    ForceHide,
}

/// One key face on the OSK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    /// A character key (letter / digit / symbol). Letters uppercase under Shift.
    Char(char),
    /// The space bar.
    Space,
    /// Delete-left.
    Backspace,
    /// Newline / submit.
    Enter,
    /// The one-shot Shift (uppercases the next letter).
    Shift,
    /// Switch to another layer (numeric / symbols / back to letters).
    Layer(Layout),
    /// Dismiss the OSK (the on-keyboard hide control).
    Dismiss,
}

/// The pure raise/dismiss decision (design lock 14): the OSK is visible exactly when
/// the device is a **Tablet** and either a text field wants keyboard input or the
/// operator forced it up — never in Laptop mode (auto-dismiss on cover re-attach).
#[must_use]
pub(crate) fn resolve_visible(formfactor: Formfactor, has_focus: bool, manual: Manual) -> bool {
    if formfactor != Formfactor::Tablet {
        return false;
    }
    match manual {
        Manual::ForceShow => true,
        Manual::ForceHide => false,
        Manual::Auto => has_focus,
    }
}

/// The keypress → egui-event mapping (design lock 14): a key injects the *same* event
/// a hardware key would, so the focused field receives it identically. Character keys
/// emit [`egui::Event::Text`] (uppercased under `shift` for letters); editing keys emit
/// a press+release [`egui::Event::Key`] pair. Layer/Shift/Dismiss keys mutate the OSK
/// itself and inject nothing.
#[must_use]
pub(crate) fn cap_events(cap: Cap, shift: bool) -> Vec<egui::Event> {
    match cap {
        Cap::Char(c) => {
            let c = if shift { c.to_ascii_uppercase() } else { c };
            vec![egui::Event::Text(c.to_string())]
        }
        Cap::Space => vec![egui::Event::Text(" ".to_string())],
        Cap::Backspace => key_press_release(egui::Key::Backspace),
        Cap::Enter => key_press_release(egui::Key::Enter),
        Cap::Shift | Cap::Layer(_) | Cap::Dismiss => Vec::new(),
    }
}

/// A hardware-identical press-then-release for a non-text key.
fn key_press_release(key: egui::Key) -> Vec<egui::Event> {
    let ev = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    vec![ev(true), ev(false)]
}

impl Layout {
    /// The key grid for this layer, row by row. Letter rows stagger like a physical
    /// keyboard; the bottom row carries the layer/space/enter controls.
    fn grid(self) -> Vec<Vec<Cap>> {
        let row = |s: &str| s.chars().map(Cap::Char).collect::<Vec<_>>();
        match self {
            Self::Qwerty => vec![
                row("qwertyuiop"),
                row("asdfghjkl"),
                with_edges(Cap::Shift, row("zxcvbnm"), Cap::Backspace),
                vec![
                    Cap::Layer(Self::Numeric),
                    Cap::Char(','),
                    Cap::Space,
                    Cap::Char('.'),
                    Cap::Enter,
                ],
            ],
            Self::Numeric => vec![
                row("1234567890"),
                row("-/:;()$&@\""),
                with_edges(Cap::Layer(Self::Symbols), row(".,?!'"), Cap::Backspace),
                vec![
                    Cap::Layer(Self::Qwerty),
                    Cap::Space,
                    Cap::Enter,
                    Cap::Dismiss,
                ],
            ],
            Self::Symbols => vec![
                row("[]{}#%^*+="),
                row("_\\|~<>"),
                with_edges(Cap::Layer(Self::Numeric), row(".,?!'"), Cap::Backspace),
                vec![
                    Cap::Layer(Self::Qwerty),
                    Cap::Space,
                    Cap::Enter,
                    Cap::Dismiss,
                ],
            ],
        }
    }
}

/// Wrap a character row with a leading + trailing control key (Shift…Backspace).
fn with_edges(lead: Cap, mut mid: Vec<Cap>, tail: Cap) -> Vec<Cap> {
    let mut row = Vec::with_capacity(mid.len() + 2);
    row.push(lead);
    row.append(&mut mid);
    row.push(tail);
    row
}

/// The label a key face shows (letters follow the live Shift state).
fn cap_label(cap: Cap, shift: bool) -> String {
    match cap {
        Cap::Char(c) => if shift { c.to_ascii_uppercase() } else { c }.to_string(),
        Cap::Space => "space".to_string(),
        Cap::Backspace => "\u{232b}".to_string(), // ⌫
        Cap::Enter => "\u{21b5}".to_string(),     // ↵
        Cap::Shift => "\u{21e7}".to_string(),     // ⇧
        Cap::Layer(Layout::Qwerty) => "ABC".to_string(),
        Cap::Layer(Layout::Numeric) => "123".to_string(),
        Cap::Layer(Layout::Symbols) => "#+=".to_string(),
        Cap::Dismiss => "\u{2304}".to_string(), // ⌄
    }
}

/// A key's width in logical points — control + space keys are wider than a letter.
fn cap_width(cap: Cap) -> f32 {
    match cap {
        Cap::Space => KEY_W * 5.0,
        Cap::Backspace | Cap::Enter | Cap::Shift | Cap::Layer(_) | Cap::Dismiss => KEY_W * 1.5,
        Cap::Char(_) => KEY_W,
    }
}

/// A key legend's rung on the HIG type ramp (PLATFORM-INTERFACES Q26): glyph faces
/// (letters / digits / symbols and the ⌫ ↵ ⇧ ⌄ controls) read at **Callout** so a
/// finger-sized cap carries a clear legend; word faces (`space` / `ABC` / `123` /
/// `#+=`) stay at **Body** so they read as controls, not oversized copy.
const fn cap_legend_size(cap: Cap) -> f32 {
    match cap {
        Cap::Char(_) | Cap::Backspace | Cap::Enter | Cap::Shift | Cap::Dismiss => {
            Style::TYPE_CALLOUT
        }
        Cap::Space | Cap::Layer(_) => Style::TYPE_BODY,
    }
}

/// A single letter key's base width (`SP_XL` + `SP_S` = 40pt) — finger-sized (lock 14).
const KEY_W: f32 = Style::SP_XL + Style::SP_S;
/// A key's height (`SP_XL` + `SP_M` = 48pt) — a comfortable touch target.
const KEY_H: f32 = Style::SP_XL + Style::SP_M;
/// The off-screen travel of the slide-in entrance, derived from the token grid.
const SLIDE: f32 = Style::SP_XL * 8.0;
/// The eframe `Id` of the OSK overlay area.
const OSK_AREA: &str = "mcnf-osk";
/// The eframe `Id` of the tablet-bar toggle affordance.
const OSK_TOGGLE: &str = "mcnf-osk-toggle";
/// The `Motion::animate` key for the slide/fade.
const OSK_ANIM: &str = "mcnf-osk-anim";
/// Shared YAMIS icon used by the tablet-bar affordance.
const OSK_TOGGLE_ICON: IconId = IconId::Keyboard;
const OSK_TOGGLE_LABEL: &str = "On-screen keyboard";

/// The shell's on-screen keyboard overlay (SURFACE-10).
///
/// Holds the cached [`Formfactor`] (fed from SURFACE-9's Bus-publish drain), the layer
/// and shift state, the manual override, and the pending injected events. Driven once
/// per frame by [`Keyboard::flush_pending`] (top of the shell render) and by
/// [`Keyboard::show`] (the overlay draw).
pub(crate) struct Keyboard {
    /// The device's current formfactor (Laptop until the seat reports a Tablet flip).
    formfactor: Formfactor,
    /// The active key layer.
    layout: Layout,
    /// One-shot Shift: uppercases the next letter, then clears.
    shift: bool,
    /// The operator's manual raise/dismiss override.
    manual: Manual,
    /// The resolved visibility from the last [`show`](Self::show) (drives the render +
    /// the toggle's show/hide sense).
    visible: bool,
    /// egui events produced by key presses, flushed into the next frame's input so the
    /// focused field consumes them exactly like a hardware key.
    pending: Vec<egui::Event>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            formfactor: Formfactor::Laptop,
            layout: Layout::default(),
            shift: false,
            manual: Manual::default(),
            visible: false,
            pending: Vec::new(),
        }
    }
}

impl Keyboard {
    /// Record a formfactor flip drained from SURFACE-9's publisher. Leaving Tablet
    /// clears any manual override so the OSK returns to the auto fold.
    pub(crate) fn set_formfactor(&mut self, formfactor: Formfactor) {
        self.formfactor = formfactor;
        if formfactor != Formfactor::Tablet {
            self.manual = Manual::Auto;
        }
    }

    /// Flush queued key events into the current frame's input (called at the very top
    /// of the shell render, before the focused field draws, so it consumes them this
    /// frame). A no-op when nothing is queued.
    pub(crate) fn flush_pending(&mut self, ctx: &egui::Context) {
        if self.pending.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.pending);
        ctx.input_mut(|i| i.events.extend(events));
    }

    /// Fold the current formfactor + focus into visibility, decaying a stale manual
    /// override once focus leaves (so a dismiss doesn't stick past the field).
    fn tick(&mut self, has_focus: bool) {
        if !has_focus && self.manual == Manual::ForceHide {
            self.manual = Manual::Auto;
        }
        self.visible = resolve_visible(self.formfactor, has_focus, self.manual);
    }

    /// Apply one pressed key: Shift/layer/dismiss mutate the OSK; everything else queues
    /// its injected events and consumes a one-shot Shift.
    fn on_cap(&mut self, cap: Cap) {
        match cap {
            Cap::Shift => self.shift = !self.shift,
            Cap::Layer(layout) => self.layout = layout,
            Cap::Dismiss => self.manual = Manual::ForceHide,
            other => {
                self.pending.extend(cap_events(other, self.shift));
                if matches!(other, Cap::Char(c) if c.is_ascii_alphabetic()) {
                    self.shift = false; // one-shot Shift clears after a letter
                }
            }
        }
    }

    /// The manual tablet-bar toggle: raise the OSK if it's down, hide it if it's up.
    const fn toggle(&mut self) {
        self.manual = if self.visible {
            Manual::ForceHide
        } else {
            Manual::ForceShow
        };
    }

    /// Draw the OSK overlay for this frame (called last in the shell render, after the
    /// surfaces, so it floats on top). Reads the live focus + drives the raise/dismiss
    /// fold, renders the tablet-bar toggle (Tablet only) and — when visible — the key
    /// grid, and restores the field's focus after any key so tapping the OSK never
    /// blurs the field being typed into.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        // Captured BEFORE the OSK draws, so it reflects the focused field (a text field
        // → wants_keyboard_input; the OSK's own buttons are not yet interacted).
        let has_focus = ctx.wants_keyboard_input();
        let focus_target = ctx.memory(egui::Memory::focused);
        self.tick(has_focus);

        // The tablet-bar affordance: only offered on a Tablet (a Laptop has its keys).
        let mut interacted = false;
        if self.formfactor == Formfactor::Tablet {
            egui::Area::new(egui::Id::new(OSK_TOGGLE))
                .order(egui::Order::Foreground)
                .anchor(
                    egui::Align2::RIGHT_BOTTOM,
                    egui::vec2(-Style::SP_M, -Style::SP_M),
                )
                .show(ctx, |ui| {
                    let r = osk_toggle_button(ui);
                    if r.clicked() {
                        self.toggle();
                        interacted = true;
                    }
                });
        }

        // Slide/fade the panel; keep drawing through the exit tween so it eases out.
        let t = Motion::animate(ctx, OSK_ANIM, self.visible, Motion::BASE);
        if self.visible || t > 0.01 {
            let slide = (1.0 - t) * SLIDE;
            let mut pressed: Option<Cap> = None;
            egui::Area::new(egui::Id::new(OSK_AREA))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, slide))
                .show(ctx, |ui| {
                    ui.set_opacity(t);
                    // Reserve a slot so the panel background paints BEHIND the keys.
                    let bg = ui.painter().add(egui::Shape::Noop);
                    let inner = ui
                        .scope(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(Style::SP_XS, Style::SP_XS);
                            self.render_rows(ui, &mut pressed);
                        })
                        .response
                        .rect
                        .expand(Style::SP_S);
                    // PLATFORM-INTERFACES Q26 — the board is a bottom sheet: RADIUS_L
                    // top corners (square bottom — it meets the screen edge), the
                    // thick material laid over the opaque BG ground (fully readable
                    // over any surface, no live blur), and a hairline edge so the
                    // sheet separates from a BG-toned surface beneath.
                    let board_radius = egui::CornerRadius {
                        nw: Style::RADIUS_L as u8,
                        ne: Style::RADIUS_L as u8,
                        sw: 0,
                        se: 0,
                    };
                    ui.painter().set(
                        bg,
                        egui::Shape::Vec(vec![
                            egui::Shape::rect_filled(inner, board_radius, Style::BG),
                            egui::Shape::rect_filled(inner, board_radius, Style::SCRIM_THICK),
                            egui::Shape::rect_stroke(
                                inner,
                                board_radius,
                                Style::hairline(),
                                egui::StrokeKind::Inside,
                            ),
                        ]),
                    );
                });
            if let Some(cap) = pressed {
                self.on_cap(cap);
                interacted = true;
            }
        }

        // A tap on the OSK's own buttons grabs egui focus; hand it straight back to the
        // field so typing continues into it (and the OSK doesn't auto-dismiss).
        if interacted {
            if let Some(id) = focus_target {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            ctx.request_repaint();
        }
    }

    /// Lay out the key grid, recording the pressed key (if any) into `pressed`.
    fn render_rows(&self, ui: &mut egui::Ui, pressed: &mut Option<Cap>) {
        // PLATFORM-INTERFACES Q26 — key caps ride the mid radius tier (RADIUS_M), a
        // rounder cap than the RADIUS_S control default. The rest/hover/pressed
        // fills stay the shared install ladder (SURFACE → SURFACE_HI → the accent
        // pressed fill), so a key press reads exactly like every platform control —
        // no private animation path.
        let key_radius = egui::CornerRadius::same(Style::RADIUS_M as u8);
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.corner_radius = key_radius;
        widgets.hovered.corner_radius = key_radius;
        widgets.active.corner_radius = key_radius;
        for row in self.layout.grid() {
            ui.horizontal(|ui| {
                for cap in row {
                    let accent = cap == Cap::Shift && self.shift;
                    // PLATFORM-INTERFACES Q26 — legends on the HIG type ramp.
                    let mut btn = egui::Button::new(
                        RichText::new(cap_label(cap, self.shift)).size(cap_legend_size(cap)),
                    );
                    if accent {
                        btn = btn.fill(Style::ACCENT);
                    }
                    if ui
                        .add_sized(egui::vec2(cap_width(cap), KEY_H), btn)
                        .clicked()
                    {
                        *pressed = Some(cap);
                    }
                }
            });
        }
    }
}

fn osk_toggle_button(ui: &mut egui::Ui) -> egui::Response {
    let response = ui.add_sized(egui::vec2(KEY_H, KEY_H), egui::Button::new(""));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), OSK_TOGGLE_LABEL)
    });
    let icon_size = Style::SP_L;
    let icon_rect =
        egui::Rect::from_center_size(response.rect.center(), egui::vec2(icon_size, icon_size));
    if let Some(tex) = icon_texture(ui.ctx(), OSK_TOGGLE_ICON, icon_size, Style::TEXT) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    } else {
        paint_keyboard_fallback(ui.painter(), icon_rect, Style::TEXT);
    }
    keyboard_hover_text(response, OSK_TOGGLE_LABEL)
}

fn keyboard_tooltip(ui: &mut egui::Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let surface = Style::resolve_color(&ctx, Style::SURFACE);
    let border = Style::resolve_color(&ctx, Style::BORDER);
    let text_color = Style::resolve_color(&ctx, Style::TEXT);
    egui::Frame::NONE
        .fill(surface)
        .stroke(egui::Stroke::new(1.0, border))
        // PLATFORM-INTERFACES Q26 — the tooltip stays on the RADIUS_S control tier
        // (same 6pt as before, now spelled from the ladder instead of a magic number).
        .corner_radius(egui::CornerRadius::same(Style::RADIUS_S as u8))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(Style::SP_XL * 12.0);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(text_color)).wrap(),
            );
        });
}

fn keyboard_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| keyboard_tooltip(ui, text.as_str()))
}

fn paint_keyboard_fallback(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.3, color);
    painter.rect_stroke(rect.shrink(1.0), 3.0, stroke, egui::StrokeKind::Inside);
    let row_h = rect.height() / 4.5;
    for row in 1..=3 {
        let y = rect.top() + row as f32 * row_h;
        painter.line_segment(
            [
                egui::pos2(rect.left() + 3.0, y),
                egui::pos2(rect.right() - 3.0, y),
            ],
            stroke,
        );
    }
    painter.line_segment(
        [
            egui::pos2(
                rect.left() + rect.width() * 0.25,
                rect.bottom() - row_h * 0.75,
            ),
            egui::pos2(
                rect.right() - rect.width() * 0.25,
                rect.bottom() - row_h * 0.75,
            ),
        ],
        egui::Stroke::new(1.8, color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)))
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

    fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    if rect.fill != egui::Color32::TRANSPARENT {
                        out.push(rect.fill);
                    }
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

    fn render_keyboard_tooltip_frame(ctx: &egui::Context) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 96.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    keyboard_tooltip(ui, OSK_TOGGLE_LABEL);
                });
            },
        )
    }

    // --- the raise/dismiss fold (formfactor × focus × manual → visible) -------------

    #[test]
    fn laptop_never_raises_even_with_focus() {
        for manual in [Manual::Auto, Manual::ForceShow, Manual::ForceHide] {
            assert!(!resolve_visible(Formfactor::Laptop, true, manual));
        }
    }

    #[test]
    fn tablet_plus_focus_auto_raises_and_dismisses() {
        assert!(resolve_visible(Formfactor::Tablet, true, Manual::Auto));
        assert!(!resolve_visible(Formfactor::Tablet, false, Manual::Auto));
    }

    #[test]
    fn manual_overrides_focus_in_tablet() {
        assert!(resolve_visible(
            Formfactor::Tablet,
            false,
            Manual::ForceShow
        ));
        assert!(!resolve_visible(
            Formfactor::Tablet,
            true,
            Manual::ForceHide
        ));
    }

    #[test]
    fn dismiss_decays_when_focus_leaves() {
        let mut kb = Keyboard::default();
        kb.set_formfactor(Formfactor::Tablet);
        kb.on_cap(Cap::Dismiss);
        assert_eq!(kb.manual, Manual::ForceHide);
        // Focus held → the dismiss sticks (no immediate re-raise).
        kb.tick(true);
        assert!(!kb.visible);
        assert_eq!(kb.manual, Manual::ForceHide);
        // Focus leaves → the override decays so the next focus auto-raises again.
        kb.tick(false);
        assert_eq!(kb.manual, Manual::Auto);
        kb.tick(true);
        assert!(kb.visible, "a fresh focus re-raises after a dismiss");
    }

    #[test]
    fn leaving_tablet_clears_a_manual_override_and_hides() {
        let mut kb = Keyboard::default();
        kb.set_formfactor(Formfactor::Tablet);
        kb.toggle(); // ForceShow (up without focus)
        assert_eq!(kb.manual, Manual::ForceShow);
        kb.tick(false);
        assert!(kb.visible);
        // Re-attaching the cover (→ Laptop) drops the override and dismisses.
        kb.set_formfactor(Formfactor::Laptop);
        assert_eq!(kb.manual, Manual::Auto);
        kb.tick(false);
        assert!(!kb.visible);
    }

    #[test]
    fn toggle_raises_then_hides() {
        let mut kb = Keyboard::default();
        kb.set_formfactor(Formfactor::Tablet);
        kb.tick(false);
        assert!(!kb.visible);
        kb.toggle();
        kb.tick(false);
        assert!(kb.visible, "manual toggle raises with no field focused");
        kb.toggle();
        kb.tick(false);
        assert!(!kb.visible, "a second toggle hides it");
    }

    #[test]
    fn tablet_toggle_uses_yamis_keyboard_icon() {
        assert_eq!(OSK_TOGGLE_ICON, IconId::Keyboard);
        assert!(
            OSK_TOGGLE_ICON.name().starts_with("yamis-"),
            "the OSK toggle should route through the YAMIS icon catalog"
        );
    }

    // --- keypress → egui-event mapping ----------------------------------------------

    fn text_of(evs: &[egui::Event]) -> Option<String> {
        evs.iter().find_map(|e| match e {
            egui::Event::Text(s) => Some(s.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_letter_injects_text_and_shift_uppercases_one_shot() {
        assert_eq!(
            text_of(&cap_events(Cap::Char('a'), false)).as_deref(),
            Some("a")
        );
        assert_eq!(
            text_of(&cap_events(Cap::Char('a'), true)).as_deref(),
            Some("A")
        );

        // One-shot Shift: a shifted letter uppercases, then Shift clears.
        let mut kb = Keyboard::default();
        kb.on_cap(Cap::Shift);
        assert!(kb.shift);
        kb.on_cap(Cap::Char('h'));
        assert_eq!(text_of(&kb.pending).as_deref(), Some("H"));
        assert!(!kb.shift, "Shift is one-shot — it clears after the letter");
    }

    #[test]
    fn editing_keys_inject_hardware_identical_key_pairs() {
        let bs = cap_events(Cap::Backspace, false);
        assert!(matches!(
            bs.as_slice(),
            [
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                },
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: false,
                    ..
                },
            ]
        ));
        let enter = cap_events(Cap::Enter, false);
        assert!(matches!(
            enter.as_slice(),
            [
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    ..
                },
                ..
            ]
        ));
        assert_eq!(
            text_of(&cap_events(Cap::Space, false)).as_deref(),
            Some(" ")
        );
        // OSK-state keys inject nothing into the field.
        assert!(cap_events(Cap::Shift, false).is_empty());
        assert!(cap_events(Cap::Layer(Layout::Numeric), false).is_empty());
        assert!(cap_events(Cap::Dismiss, false).is_empty());
    }

    #[test]
    fn flush_moves_pending_into_the_frame_input() {
        let mut kb = Keyboard::default();
        kb.on_cap(Cap::Char('x'));
        assert!(!kb.pending.is_empty());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            kb.flush_pending(ctx);
            // The queued Text event is now visible to widgets drawn this frame.
            let seen = ctx.input(|i| {
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(s) if s == "x"))
            });
            assert!(seen, "flush injected the key into the frame input");
        });
        assert!(kb.pending.is_empty(), "flush drains the queue");
    }

    // --- the layout state machine ---------------------------------------------------

    #[test]
    fn layers_toggle_and_each_grid_is_non_empty() {
        let mut kb = Keyboard::default();
        assert_eq!(kb.layout, Layout::Qwerty);
        kb.on_cap(Cap::Layer(Layout::Numeric));
        assert_eq!(kb.layout, Layout::Numeric);
        kb.on_cap(Cap::Layer(Layout::Symbols));
        assert_eq!(kb.layout, Layout::Symbols);
        kb.on_cap(Cap::Layer(Layout::Qwerty));
        assert_eq!(kb.layout, Layout::Qwerty);

        for layout in [Layout::Qwerty, Layout::Numeric, Layout::Symbols] {
            let grid = layout.grid();
            assert!(
                !grid.is_empty() && grid.iter().all(|r| !r.is_empty()),
                "{layout:?}"
            );
            // Every layer offers Backspace, Enter and Space.
            let flat: Vec<Cap> = grid.into_iter().flatten().collect();
            for needed in [Cap::Backspace, Cap::Enter, Cap::Space] {
                assert!(flat.contains(&needed), "{layout:?} lacks {needed:?}");
            }
        }
    }

    // --- the overlay actually renders (headless paint path) -------------------------

    #[test]
    fn the_key_grid_tessellates_a_real_layout() {
        // Drive the OSK's key grid through the same CPU paint→tessellate path the DRM
        // runner uses (minus the GPU): a live layout must produce real draw primitives,
        // proving the overlay renders rather than being dead code. (`show`'s slide/fade
        // opacity is time-driven, so we exercise the content render directly here; the
        // raise/dismiss + injection folds above cover the rest of `show`'s wiring.)
        let kb = Keyboard::default();
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1024.0, 768.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut pressed = None;
                kb.render_rows(ui, &mut pressed);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the OSK key grid drew nothing");
    }

    #[test]
    fn osk_toggle_tooltip_uses_themed_text_and_surface() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = render_keyboard_tooltip_frame(&ctx);

        let texts = painted_text_colors(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == OSK_TOGGLE_LABEL && *color == Style::TEXT),
            "OSK toggle tooltip should paint themed text: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|(text, color)| text == OSK_TOGGLE_LABEL && *color == egui::Color32::BLACK),
            "OSK toggle tooltip leaked raw black popup text: {texts:?}"
        );

        let fills = painted_fill_colors(&out.shapes);
        assert!(
            fills.contains(&Style::SURFACE),
            "OSK toggle tooltip should paint its themed surface: {fills:?}"
        );
    }

    #[test]
    fn show_drives_without_panicking_and_self_gates_on_laptop() {
        // A full `show` pass on a Laptop stays inert (nothing raised) and never panics —
        // the honest windowed/dev default (§7). Also exercises the Tablet toggle path.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1024.0, 768.0),
            )),
            ..Default::default()
        };
        let mut kb = Keyboard::default();
        let _ = ctx.run(input(), |ctx| kb.show(ctx));
        assert!(!kb.visible, "the OSK stays down on a Laptop");
        // On a Tablet with a manual raise, a `show` pass marks it visible.
        kb.set_formfactor(Formfactor::Tablet);
        kb.toggle();
        let _ = ctx.run(input(), |ctx| kb.show(ctx));
        assert!(kb.visible, "a manual raise shows the OSK on a Tablet");
    }
}
