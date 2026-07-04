//! EDTB-1 (part B) — the **Word-97 Standard toolbar**: the button strip below
//! the menu bar (design: `docs/design/editor-toolbar.md`).
//!
//! Word's Standard-toolbar order, cut to the controls whose backends exist
//! today (lock #4 — no dead buttons): `New · Open · Save · | · Cut · Copy ·
//! Paste · | · Undo · Redo · | · Zoom`. The omitted Word buttons await their
//! phases: Print / Print Preview (EDTB-5), Spelling (EDTB-6), Insert Table +
//! `AutoFormat` (EDTB-3/2), Format Painter (no rich-text analogue — out of
//! scope). Every button dispatches the **same** [`MenuAction`] its menu twin
//! does — one dispatch seam, zero duplication (§6).
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

use crate::menu_bar::{Gate, MenuAction, MenuContext};

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

/// The Standard strip in Word-97 order, as data — the render is one thin loop,
/// and [`tests`] assert the order + that every button drives a real action.
pub const STRIP: [StripEntry; 12] = [
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
pub fn show(ui: &mut Ui, cx: &MenuContext) -> Option<MenuAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        for entry in &STRIP {
            match entry {
                StripEntry::Separator => {
                    ui.separator();
                }
                StripEntry::Button(button) => {
                    let resp = ui
                        .add_enabled(
                            button.gate.enabled(cx),
                            Button::new(RichText::new(button.glyph).size(Style::BODY)),
                        )
                        .on_hover_text(button.name)
                        .on_disabled_hover_text(button.name);
                    if resp.clicked() {
                        action = Some(button.action);
                    }
                }
                StripEntry::Zoom => {
                    // No document → no zoom control (nothing to zoom); the
                    // dropdown reappears with the next open document.
                    if let Some(percent) = cx.zoom_percent {
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
                    }
                }
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::{MenuAction, StripEntry, STRIP, ZOOM_STEPS};

    #[test]
    fn the_strip_is_the_word_97_standard_order() {
        // New · Open · Save · | · Cut · Copy · Paste · | · Undo · Redo · | ·
        // Zoom — the exact EDTB-1 strip (Print/Spell/Table await their phases).
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
                "New", "Open", "Save", "|", "Cut", "Copy", "Paste", "|", "Undo", "Redo", "|",
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
    fn zoom_steps_are_the_word_ladder_and_include_default() {
        assert_eq!(ZOOM_STEPS, [50, 75, 100, 125, 150, 200]);
        assert!(ZOOM_STEPS.contains(&100), "the default zoom is offered");
        assert!(
            ZOOM_STEPS.windows(2).all(|w| w[0] < w[1]),
            "the ladder ascends"
        );
    }
}
