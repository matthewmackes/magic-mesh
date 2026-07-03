//! EDITOR-7 (part B) — the **command palette overlay**: an overlay listing the
//! named editor commands, fuzzy-filtered, Enter to run.
//!
//! `Cmd`/`Ctrl-Shift-P` toggles it (the intercept lives at the panel level in
//! [`crate::panel`], never in the text widget). Each entry is a real command over
//! the existing [`EditorSurface`](crate::EditorSurface) seams — Save, Open Scratch,
//! Toggle Project Tree, Toggle Soft-Wrap, Close Document, Open Folder — dispatched
//! by [`EditorSurface::run_command`](crate::EditorSurface::run_command). There are
//! **no dead entries** (§7): picking one actually invokes its seam.
//!
//! The command set + titles + filtering ([`PaletteCommand`], the ranked
//! [`results`](CommandPalette::results), selection movement, [`pick`](CommandPalette::pick))
//! are pure and unit-tested without egui; [`show`] is a thin token-styled (§4)
//! render, and the dispatch lives in [`crate::panel`] where it can reach the
//! surface's private document.

// `module_name_repetitions`: `CommandPalette` / `PaletteCommand` are the domain
// names for this module's public types; trimming the echo of the `palette` module
// reads worse. `missing_const_for_fn` (nursery) is over-eager on the small
// mutators — the same allow `buffer.rs` makes.
#![allow(clippy::module_name_repetitions, clippy::missing_const_for_fn)]

use mde_egui::egui::{self, Align2, Key, Modifiers, RichText, Vec2};
use mde_egui::Style;

use crate::fuzzy;

/// Fixed width of the palette overlay plate (§4 spacing units).
const PALETTE_WIDTH: f32 = Style::SP_XL * 14.0;
/// Vertical drop of the overlay from the top edge (§4 spacing units).
const TOP_DROP: f32 = Style::SP_XL * 2.0;

/// One named editor command the palette offers. Each maps to a real seam on
/// [`EditorSurface`](crate::EditorSurface), dispatched by
/// [`run_command`](crate::EditorSurface::run_command) — no dead entries (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    /// Write the open document to disk (`buffer.save`).
    Save,
    /// Open a fresh scratch buffer.
    OpenScratch,
    /// Show/hide the project-tree side panel.
    ToggleTree,
    /// Flip the editor's soft-wrap.
    ToggleWrap,
    /// Close the open document, returning to the empty state.
    CloseDoc,
    /// Root the project tree at the current working directory.
    OpenFolderCwd,
}

impl PaletteCommand {
    /// Every command the palette offers, in display order.
    pub const ALL: [Self; 6] = [
        Self::Save,
        Self::OpenScratch,
        Self::ToggleTree,
        Self::ToggleWrap,
        Self::CloseDoc,
        Self::OpenFolderCwd,
    ];

    /// The human title shown in the palette and matched against the query.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::OpenScratch => "Open Scratch Buffer",
            Self::ToggleTree => "Toggle Project Tree",
            Self::ToggleWrap => "Toggle Soft-Wrap",
            Self::CloseDoc => "Close Document",
            Self::OpenFolderCwd => "Open Folder (current directory)",
        }
    }

    /// A short, dimmed subtitle naming the seam the command drives — keeps the
    /// palette honest about what each entry actually does (§7).
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Save => "buffer.save",
            Self::OpenScratch => "new scratch document",
            Self::ToggleTree => "project tree panel",
            Self::ToggleWrap => "editor soft-wrap",
            Self::CloseDoc => "back to empty state",
            Self::OpenFolderCwd => "root tree at cwd",
        }
    }
}

/// The command-palette overlay state (EDITOR-7).
///
/// Holds only the open flag, the live query, and the highlighted row — the command
/// set itself is the static [`PaletteCommand::ALL`]. `toggle`/`close`/`results`/`pick`
/// are pure state so the filtering is unit-tested without egui; [`show`] renders it.
#[derive(Default)]
pub struct CommandPalette {
    /// Whether the overlay is currently shown.
    open: bool,
    /// The live query text.
    query: String,
    /// The highlighted row — an index into the *ranked results*, not `ALL`.
    selected: usize,
    /// Set on open so the query field grabs the keyboard for one frame.
    focus_query: bool,
}

impl CommandPalette {
    /// Whether the palette overlay is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Toggle the palette (the `Cmd`/`Ctrl-Shift-P` intercept). Opening resets the
    /// query + selection and requests keyboard focus.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.focus_query = true;
        }
    }

    /// Close the overlay (Esc, or after a pick).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The command indices matching the query, best-first (the ranked results).
    fn results(&self) -> Vec<usize> {
        fuzzy::ranked(
            &self.query,
            PaletteCommand::ALL
                .iter()
                .enumerate()
                .map(|(idx, cmd)| (idx, cmd.title())),
        )
    }

    /// Move the highlighted row one step within `len` results, saturating at the
    /// ends (the list doesn't wrap).
    fn move_selection(&mut self, forward: bool, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let last = len - 1;
        let cur = self.selected.min(last);
        self.selected = if forward {
            (cur + 1).min(last)
        } else {
            cur.saturating_sub(1)
        };
    }

    /// The highlighted command, if any — what Enter / a click runs.
    fn pick(&self, results: &[usize]) -> Option<PaletteCommand> {
        results
            .get(self.selected)
            .and_then(|&idx| PaletteCommand::ALL.get(idx))
            .copied()
    }
}

/// Render the palette overlay on `ctx` and return the command the operator ran
/// this frame (Enter on the highlighted row, or a click), closing the overlay on a
/// run or Esc. A no-op returning `None` while the overlay is closed.
///
/// The nav chords are consumed here — before the query field reads them — so the
/// arrows move the selection (not the text caret), Enter runs, and Esc dismisses.
pub fn show(ctx: &egui::Context, palette: &mut CommandPalette) -> Option<PaletteCommand> {
    if !palette.open {
        return None;
    }

    let results = palette.results();
    if palette.selected >= results.len() {
        palette.selected = results.len().saturating_sub(1);
    }

    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        palette.close();
        return None;
    }
    if up {
        palette.move_selection(false, results.len());
    }
    if down {
        palette.move_selection(true, results.len());
    }

    let mut picked = if enter { palette.pick(&results) } else { None };

    egui::Window::new("Command Palette")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_DROP))
        .show(ctx, |ui| {
            ui.set_min_width(PALETTE_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Command Palette")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);

            let field = ui.add(
                egui::TextEdit::singleline(&mut palette.query)
                    .hint_text("Type a command\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut palette.focus_query) {
                field.request_focus();
            }
            ui.add_space(Style::SP_XS);
            ui.separator();

            if results.is_empty() {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new("No matching commands")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .italics(),
                );
                return;
            }

            for (row, &idx) in results.iter().enumerate() {
                let cmd = PaletteCommand::ALL[idx];
                if command_row(ui, cmd, row == palette.selected) {
                    palette.selected = row;
                    picked = Some(cmd);
                }
            }
        });

    if picked.is_some() {
        palette.close();
    }
    picked
}

/// One command row: the title (selectable / highlighted when picked) plus its
/// dimmed seam hint. Returns `true` when clicked. Token-styled (§4).
fn command_row(ui: &mut egui::Ui, cmd: PaletteCommand, selected: bool) -> bool {
    ui.horizontal(|ui| {
        let clicked = ui
            .selectable_label(
                selected,
                RichText::new(cmd.title())
                    .size(Style::BODY)
                    .color(Style::TEXT),
            )
            .clicked();
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(cmd.hint())
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        clicked
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::{CommandPalette, PaletteCommand};

    #[test]
    fn every_command_has_a_nonempty_title_and_hint() {
        // §7 — no dead / blank entries: each of the six commands is named + hinted.
        assert_eq!(
            PaletteCommand::ALL.len(),
            6,
            "the full command set is listed"
        );
        for cmd in PaletteCommand::ALL {
            assert!(!cmd.title().is_empty(), "{cmd:?} has a title");
            assert!(!cmd.hint().is_empty(), "{cmd:?} has a seam hint");
        }
    }

    #[test]
    fn an_empty_query_lists_every_command_in_order() {
        let palette = CommandPalette::default();
        // `default` is closed; results only reads query/ALL, so it lists all six.
        let results = palette.results();
        assert_eq!(
            results,
            vec![0, 1, 2, 3, 4, 5],
            "empty query lists every command"
        );
    }

    #[test]
    fn a_query_filters_and_ranks_to_the_matching_command() {
        // `default` selects row 0, so `pick` returns the top-ranked match.
        let palette = CommandPalette {
            query: "wrap".to_owned(),
            ..CommandPalette::default()
        };
        let results = palette.results();
        assert_eq!(
            palette.pick(&results),
            Some(PaletteCommand::ToggleWrap),
            "'wrap' resolves to Toggle Soft-Wrap"
        );

        let palette = CommandPalette {
            query: "scratch".to_owned(),
            ..CommandPalette::default()
        };
        let results = palette.results();
        assert_eq!(palette.pick(&results), Some(PaletteCommand::OpenScratch));
    }

    #[test]
    fn toggle_opens_then_closes_and_resets() {
        let mut palette = CommandPalette::default();
        assert!(!palette.is_open());
        palette.selected = 3;
        palette.toggle();
        assert!(palette.is_open(), "toggle opens");
        assert_eq!(palette.selected, 0, "opening resets the selection");
        palette.toggle();
        assert!(!palette.is_open(), "toggle again closes");
    }

    #[test]
    fn a_non_matching_query_yields_no_command() {
        let palette = CommandPalette {
            query: "zzzzz".to_owned(),
            ..CommandPalette::default()
        };
        let results = palette.results();
        assert!(results.is_empty(), "no command matches gibberish");
        assert_eq!(palette.pick(&results), None, "nothing to pick");
    }
}
