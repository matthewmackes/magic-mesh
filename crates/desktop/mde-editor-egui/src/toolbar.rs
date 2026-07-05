//! EDTB-1 (part B) — the **Word-97 Standard toolbar**: the button strip below
//! the menu bar (design: `docs/design/editor-toolbar.md`).
//!
//! Word's Standard-toolbar order, cut to the controls whose backends exist
//! today (lock #4 — no dead buttons): `New · Open · Save · | · Print · Print
//! Preview · | · Cut · Copy · Paste · | · Undo · Redo · | · Zoom`. The remaining
//! omitted Word buttons await their phases: Spelling (EDTB-6), Format Painter (no
//! rich-text analogue — out of scope). The Print group landed in EDTB-5. Every
//! button dispatches the **same** [`MenuAction`] its menu twin does — one dispatch
//! seam, zero duplication (§6).
//!
//! Glyphs are text glyphs from the workspace's established vocabulary (egui's
//! built-in faces stay installed as fallback behind Droid Sans Mono, so the
//! symbol/emoji points render); no new SVG assets this task — the iced-era
//! `IconId` set lives in the retiring `mde-theme` crate, which this egui
//! surface deliberately does not depend on. Unlike the shell bar (whose rule is
//! *no tooltips*), the editor toolbar uses small Word-style hover tooltips
//! naming each command; hover fill comes from the shared theme's widget visuals
//! (§4).
//!
//! The **Zoom dropdown** (50–200%, Word's steps) drives a *real* editor-view
//! font scale: [`MenuAction::Zoom`] routes to
//! [`EditorView::set_zoom_percent`](crate::EditorView::set_zoom_percent), and
//! the widget derives every cell metric from the scaled font. With no open
//! document the dropdown is omitted — there is nothing to zoom.

use mde_egui::egui::{self, Button, RichText, Ui};
use mde_egui::Style;

use crate::menu_bar::{Gate, MenuAction, MenuContext, OVERFLOW_GLYPH};

/// The Word-97 zoom steps the dropdown offers.
pub const ZOOM_STEPS: [u16; 6] = [50, 75, 100, 125, 150, 200];

/// One Standard-toolbar button: its glyph, the command name (the Word-style
/// tooltip), the shared action it dispatches, and its enablement gate.
pub struct ToolButton {
    /// The button face — a text glyph (no new SVGs this task).
    pub glyph: &'static str,
    /// The command name, shown as the hover tooltip.
    pub name: &'static str,
    /// The dispatched action — the same one the menu twin sends.
    pub action: MenuAction,
    /// The enablement gate (shared with the menu bar).
    pub gate: Gate,
}

impl ToolButton {
    /// Shorthand constructor for the static strip below.
    const fn new(glyph: &'static str, name: &'static str, action: MenuAction, gate: Gate) -> Self {
        Self {
            glyph,
            name,
            action,
            gate,
        }
    }
}

/// One slot in the strip: a button, a Word group separator, or the zoom
/// dropdown.
pub enum StripEntry {
    /// A command button.
    Button(ToolButton),
    /// A vertical group separator.
    Separator,
    /// The zoom dropdown (rendered only with an open document).
    Zoom,
}

impl StripEntry {
    /// Whether this entry folds into the compact `»` overflow (EDTB-4) rather
    /// than rendering inline when the panel is narrow. Only the width-heavy
    /// **Zoom** dropdown folds — its `100%` text label is the one wide control on
    /// this strip; the icon buttons and group separators are already narrow, so
    /// they always stay in line (the compact form the strip needs). Pure so the
    /// §7 "no command lost" fold split is unit-testable.
    const fn folds_in_compact(&self) -> bool {
        matches!(self, Self::Zoom)
    }
}

/// The Standard strip in Word-97 order, as data — the render is one thin loop,
/// and [`tests`] assert the order + that every button drives a real action.
pub const STRIP: [StripEntry; 17] = [
    StripEntry::Button(ToolButton::new(
        "\u{FF0B}",
        "New",
        MenuAction::NewScratch,
        Gate::Always,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{1F4C2}",
        "Open",
        MenuAction::OpenFinder,
        Gate::Always,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{1F4BE}",
        "Save",
        MenuAction::Save,
        Gate::Doc,
    )),
    StripEntry::Separator,
    // The EDTB-5 Print group (Word's Standard-toolbar Print · Print Preview),
    // between Save and the clipboard group; both need an open document to print.
    StripEntry::Button(ToolButton::new(
        "\u{1F5A8}",
        "Print",
        MenuAction::Print,
        Gate::Doc,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{1F50D}",
        "Print Preview",
        MenuAction::PrintPreview,
        Gate::Doc,
    )),
    StripEntry::Separator,
    // The EDTB-6 Spelling button (Word's Standard-toolbar Spelling & Grammar),
    // between the Print and clipboard groups. Greyed unless hunspell is installed
    // and the buffer is a spell-checkable (md/text) type (Gate::Spell).
    StripEntry::Button(ToolButton::new(
        "\u{2713}",
        "Spelling (F7)",
        MenuAction::SpellCheck,
        Gate::Spell,
    )),
    StripEntry::Separator,
    StripEntry::Button(ToolButton::new(
        "\u{2702}",
        "Cut",
        MenuAction::Cut,
        Gate::Selection,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{29C9}",
        "Copy",
        MenuAction::Copy,
        Gate::Selection,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{1F4CB}",
        "Paste",
        MenuAction::Paste,
        Gate::Doc,
    )),
    StripEntry::Separator,
    StripEntry::Button(ToolButton::new(
        "\u{21BA}",
        "Undo",
        MenuAction::Undo,
        Gate::UndoStack,
    )),
    StripEntry::Button(ToolButton::new(
        "\u{21BB}",
        "Redo",
        MenuAction::Redo,
        Gate::RedoStack,
    )),
    StripEntry::Separator,
    StripEntry::Zoom,
];

/// Render the Standard toolbar and return the action the operator picked this
/// frame, if any. Buttons grey out by context exactly like their menu twins;
/// each carries its command name as a small hover tooltip (Word-style).
///
/// When `compact` (EDTB-4 — the panel is narrow), the width-heavy Zoom dropdown
/// folds into a trailing `»` overflow instead of rendering inline, so the strip
/// leans out to its narrow icon buttons while keeping every command reachable
/// (§7 — relocated, never lost). At full width every control renders in line.
pub fn show(ui: &mut Ui, cx: &MenuContext, compact: bool) -> Option<MenuAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        for entry in &STRIP {
            // Compact folds the width-heavy controls into the `»` overflow
            // (rendered after the loop); the narrow icon buttons + separators
            // always stay in line.
            if compact && entry.folds_in_compact() {
                continue;
            }
            match entry {
                StripEntry::Separator => {
                    ui.separator();
                }
                StripEntry::Button(button) => {
                    if let Some(picked) = tool_button(ui, cx, button) {
                        action = Some(picked);
                    }
                }
                StripEntry::Zoom => {
                    if let Some(picked) = zoom_dropdown(ui, cx) {
                        action = Some(picked);
                    }
                }
            }
        }
        // EDTB-4 — the compact `»` overflow: the folded width-heavy controls,
        // still fully reachable (§7). Omitted with nothing to fold.
        if compact {
            if let Some(picked) = overflow(ui, cx) {
                action = Some(picked);
            }
        }
    });
    action
}

/// Render one Standard-toolbar command button (icon-only face + Word tooltip),
/// greyed by its context gate exactly like its menu twin. Returns its action if
/// the operator clicked it this frame.
fn tool_button(ui: &mut Ui, cx: &MenuContext, button: &ToolButton) -> Option<MenuAction> {
    let resp = ui
        .add_enabled(
            button.gate.enabled(cx),
            Button::new(RichText::new(button.glyph).size(Style::BODY)),
        )
        .on_hover_text(button.name)
        .on_disabled_hover_text(button.name);
    resp.clicked().then_some(button.action)
}

/// Render the Zoom dropdown (Word's 50–200% steps) inline — but only with an open
/// document: with none there is nothing to zoom, so it is omitted (the dropdown
/// reappears with the next open document). Returns the picked `MenuAction::Zoom`.
fn zoom_dropdown(ui: &mut Ui, cx: &MenuContext) -> Option<MenuAction> {
    let percent = cx.zoom_percent?;
    let mut action = None;
    egui::ComboBox::from_id_salt("editor-zoom")
        .selected_text(format!("{percent}%"))
        .show_ui(ui, |ui| {
            for step in ZOOM_STEPS {
                if ui
                    .selectable_label(step == percent, format!("{step}%"))
                    .clicked()
                {
                    action = Some(MenuAction::Zoom(step));
                }
            }
        })
        .response
        .on_hover_text("Zoom");
    action
}

/// The compact `»` overflow menu: the width-heavy controls the narrow strip
/// folded out of line, still fully reachable (§7 — relocated, never lost). On the
/// Standard strip that is the Zoom dropdown; it renders here as the same zoom
/// steps, emitting the same [`MenuAction::Zoom`] the inline dropdown does (§6).
/// Omitted (no `»`) when nothing folds — i.e. with no open document to zoom.
fn overflow(ui: &mut Ui, cx: &MenuContext) -> Option<MenuAction> {
    let percent = cx.zoom_percent?;
    let mut action = None;
    ui.menu_button(OVERFLOW_GLYPH, |ui| {
        ui.label(
            RichText::new("Zoom")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        for step in ZOOM_STEPS {
            if ui
                .selectable_label(step == percent, format!("{step}%"))
                .clicked()
            {
                action = Some(MenuAction::Zoom(step));
                ui.close_menu();
            }
        }
    })
    .response
    .on_hover_text("More controls");
    action
}

#[cfg(test)]
mod tests {
    use super::{MenuAction, StripEntry, STRIP, ZOOM_STEPS};

    #[test]
    fn the_strip_is_the_word_97_standard_order() {
        // New · Open · Save · | · Print · Print Preview · | · Spelling · | · Cut ·
        // Copy · Paste · | · Undo · Redo · | · Zoom — the EDTB-1 strip with the
        // EDTB-5 Print group + the EDTB-6 Spelling button.
        let shape: Vec<String> = STRIP
            .iter()
            .map(|e| match e {
                StripEntry::Button(b) => b.name.to_owned(),
                StripEntry::Separator => "|".to_owned(),
                StripEntry::Zoom => "Zoom".to_owned(),
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                "New",
                "Open",
                "Save",
                "|",
                "Print",
                "Print Preview",
                "|",
                "Spelling (F7)",
                "|",
                "Cut",
                "Copy",
                "Paste",
                "|",
                "Undo",
                "Redo",
                "|",
                "Zoom"
            ]
        );
    }

    #[test]
    fn every_button_has_a_glyph_a_name_and_a_real_action() {
        for entry in &STRIP {
            if let StripEntry::Button(b) = entry {
                assert!(!b.glyph.is_empty(), "{} has no glyph", b.name);
                assert!(!b.name.is_empty(), "a button has no tooltip name");
                // No Zoom(_) hides among the buttons (zoom is the dropdown),
                // and no button ships a phase that hasn't landed.
                assert!(
                    !matches!(b.action, MenuAction::Zoom(_)),
                    "{} misroutes to Zoom",
                    b.name
                );
            }
        }
    }

    #[test]
    fn button_actions_match_their_menu_twins() {
        // §6 — one dispatch seam: each toolbar button sends exactly the action
        // its menu-bar twin sends, so both drive the same surface fn.
        let expect = [
            ("New", MenuAction::NewScratch),
            ("Open", MenuAction::OpenFinder),
            ("Save", MenuAction::Save),
            ("Print", MenuAction::Print),
            ("Print Preview", MenuAction::PrintPreview),
            ("Spelling (F7)", MenuAction::SpellCheck),
            ("Cut", MenuAction::Cut),
            ("Copy", MenuAction::Copy),
            ("Paste", MenuAction::Paste),
            ("Undo", MenuAction::Undo),
            ("Redo", MenuAction::Redo),
        ];
        let buttons: Vec<(&str, MenuAction)> = STRIP
            .iter()
            .filter_map(|e| match e {
                StripEntry::Button(b) => Some((b.name, b.action)),
                _ => None,
            })
            .collect();
        assert_eq!(buttons, expect);
    }

    #[test]
    fn only_the_zoom_dropdown_folds_into_the_compact_overflow() {
        // EDTB-4 §7 — the narrow icon buttons + separators always stay inline;
        // only the width-heavy Zoom dropdown folds into the `»` overflow. Nothing
        // is dropped — every command button is reachable inline in both layouts.
        let folded: Vec<&StripEntry> = STRIP.iter().filter(|e| e.folds_in_compact()).collect();
        assert_eq!(folded.len(), 1, "exactly the Zoom dropdown folds");
        assert!(matches!(folded[0], StripEntry::Zoom));
        for entry in &STRIP {
            if let StripEntry::Button(b) = entry {
                assert!(
                    !entry.folds_in_compact(),
                    "the {} button must not fold — common commands stay inline",
                    b.name
                );
            }
        }
    }

    #[test]
    fn zoom_steps_are_the_word_ladder_and_include_default() {
        assert_eq!(ZOOM_STEPS, [50, 75, 100, 125, 150, 200]);
        assert!(ZOOM_STEPS.contains(&100), "the default zoom is offered");
        assert!(
            ZOOM_STEPS.windows(2).all(|w| w[0] < w[1]),
            "the ladder ascends"
        );
    }
}
