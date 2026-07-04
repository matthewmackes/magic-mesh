//! EDTB-1 (part A) — the **Word-97 menu bar** across the top of the editor
//! panel (design: `docs/design/editor-toolbar.md`).
//!
//! An `egui::menu::bar` of the classic Word menus, adapted honestly to what the
//! editor can actually do today. Per lock #4 (**no dead entries** — an item
//! appears only when its seam exists), this phase ships **seven** of the eight
//! Word-97 menus:
//!
//! * **File** — New (scratch), Open… (the `Ctrl-P` finder), Open Folder (cwd →
//!   project tree), Save, Save As… (a small path dialog), Close.
//! * **Edit** — Undo, Redo, Cut, Copy, Paste, Select All. **Find… is omitted**:
//!   in-buffer find is EDITOR-8 and has not landed, so there is no honest
//!   target yet (the finder finds *files*, not text — routing Find there would
//!   lie about what it does).
//! * **View** — Project Tree + Terminal + Soft-Wrap toggles (checked = current state).
//!   Preview / per-bar toggles arrive with EDTB-7 / later phases.
//! * **Insert** (EDTB-3) — Table…, which opens the Word grid-picker and drops a
//!   markdown table skeleton at the caret ([`crate::md_actions::insert_table`]).
//! * **Format** (EDTB-3) — the menu twin of the Formatting strip: Bold / Italic
//!   / Underline / Strikethrough ([`crate::md_actions::toggle_wrap`]), Bullet /
//!   Numbered list ([`toggle_line_prefix`](crate::md_actions::toggle_line_prefix)),
//!   Increase / Decrease Indent ([`shift_indent`](crate::md_actions::shift_indent)),
//!   and Normal / Heading 1-6 ([`set_heading`](crate::md_actions::set_heading)) —
//!   keyboard/menu parity with the strip, one dispatch seam (§6).
//! * **Tools** — Command Palette… (`Ctrl-Shift-P`).
//! * **Help** — About the Editor (crate name + the workspace version line;
//!   `mde-egui` exposes no brand/build module, so the crate's own
//!   `CARGO_PKG_VERSION` — the workspace-inherited platform version — is the
//!   honest source).
//!
//! **Omitted** (they appear as their phases land, per lock #4): the standalone
//! **Table** menu (cell/row operations — not yet backed; Insert → Table… covers
//! creation) and the Print/Spell items in File/Tools (P2/P3 — EDTB-5/6).
//!
//! The menu tree is **data** ([`MENUS`]) rendered by one thin loop, so the §7
//! guarantees are unit-testable without egui: every item maps to a real
//! [`MenuAction`] the surface dispatches ([`crate::EditorSurface`]'s
//! `run_action`), no menu is empty, and the omitted menus/items are asserted
//! absent. Items whose precondition is missing (Save with no document, Cut with
//! no selection, Undo with an empty log) render **disabled** — the authentic
//! Word-97 grey-out; the backends all exist, only the context gates them.
//!
//! Labels render theme-styled (no forced `RichText` color) so egui's disabled
//! dimming works; the theme itself is the shared Carbon [`Style`] install (§4).

use mde_egui::egui::{self, Button, Ui};
use mde_egui::Style;

/// Minimum dropdown width so short menus don't collapse into slivers — six
/// shared spacing units (§4), the same derivation as the panel's tree width.
const MENU_MIN_W: f32 = Style::SP_XL * 6.0;

/// The compact **overflow** affordance's glyph — Word's `»` chevrons, the "more
/// controls" menu the Standard/Formatting strips fold their width-heavy dropdown
/// into when the editor panel goes narrow (EDTB-4). Shared by both toolbars so
/// the fold reads identically on each strip. The menu bar itself never overflows
/// (it is already compact — dropdown buttons), so this lives here only as the
/// bars' shared vocabulary, alongside [`MenuAction`]/[`MenuContext`].
pub const OVERFLOW_GLYPH: &str = "\u{00BB}";

/// One action a menu item or Standard-toolbar button dispatches — the EDTB-1
/// analogue of [`PaletteCommand`](crate::palette::PaletteCommand). Every variant
/// routes to a real seam in `EditorSurface::run_action` (§7 — no dead entries);
/// where the palette already names the command, the dispatch calls the *same*
/// fn (§6, no duplication).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Open a fresh scratch buffer (the palette's Open Scratch seam).
    NewScratch,
    /// Open the fuzzy file finder — the same overlay `Ctrl-P` raises.
    OpenFinder,
    /// Root the project tree at the cwd (the palette's Open Folder seam).
    OpenFolderCwd,
    /// Write the open document to disk (the palette's Save seam).
    Save,
    /// Open the Save As… path dialog.
    SaveAs,
    /// Close the open document (the palette's Close seam).
    CloseDoc,
    /// Undo one widget step (`EditorView::undo`).
    Undo,
    /// Redo one widget step (`EditorView::redo`).
    Redo,
    /// Copy the selection to the clipboard, then delete it.
    Cut,
    /// Copy the selection to the clipboard.
    Copy,
    /// Ask the platform for a paste — arrives as the same `Event::Paste` the
    /// widget already inserts.
    Paste,
    /// Select the whole buffer (`EditorView::select_all`).
    SelectAll,
    /// Show/hide the project-tree side panel (the palette's toggle).
    ToggleTree,
    /// Show/hide the integrated terminal dock (EDITOR-10; the palette's toggle).
    ToggleTerminal,
    /// Flip the editor's soft-wrap (the palette's toggle).
    ToggleWrap,
    /// Toggle the command palette — the same overlay `Ctrl-Shift-P` raises.
    CommandPalette,
    /// Open the About dialog.
    About,
    /// Set the editor-view zoom to this percent (the toolbar dropdown).
    Zoom(u16),
    /// Set the selected lines' markdown heading level (0 = Normal body text,
    /// 1-6 = `#`..`######`) — the Format strip Style dropdown + the Format menu
    /// ([`crate::md_actions::set_heading`]).
    Heading(u8),
    /// Toggle a markdown inline wrap around every caret's selection — the Format
    /// strip B/I/U/S buttons + the Format menu
    /// ([`crate::md_actions::toggle_wrap`]).
    Wrap(WrapMarker),
    /// Toggle a list prefix on the selected lines — the Format strip list
    /// buttons + the Format menu
    /// ([`crate::md_actions::toggle_line_prefix`]).
    List(ListStyle),
    /// Shift the selected lines' indent by this many two-space levels (±1) — the
    /// Format strip indent buttons + the Format menu
    /// ([`crate::md_actions::shift_indent`]).
    Indent(i8),
    /// Open the Insert Table grid-picker (Insert → Table…).
    InsertTablePicker,
    /// Insert a `rows`×`cols` markdown table skeleton at the caret — what the
    /// grid-picker commits, routed through the shared dispatch for menu/test
    /// parity ([`crate::md_actions::insert_table`]).
    InsertTable {
        /// Body rows below the header row.
        rows: u8,
        /// Columns.
        cols: u8,
    },
}

/// A markdown inline wrap the Format controls toggle (design lock #1). Each maps
/// to the marker string [`crate::md_actions::toggle_wrap`] wraps the selection
/// with; underline has no markdown form, so it uses honest inline HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMarker {
    /// `**bold**`.
    Bold,
    /// `*italic*`.
    Italic,
    /// `<u>underline</u>` — honest HTML-in-md (markdown has no underline).
    Underline,
    /// `~~strikethrough~~`.
    Strike,
}

impl WrapMarker {
    /// The markdown marker string the engine wraps a selection with.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Bold => "**",
            Self::Italic => "*",
            Self::Underline => "<u>",
            Self::Strike => "~~",
        }
    }
}

/// A list style the Format controls toggle (design lock #8) — mapped to the
/// engine's `ListKind` at the dispatch (keeping this action vocabulary free of
/// the engine module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyle {
    /// `- ` bullets.
    Bullet,
    /// `1. ` numbers.
    Numbered,
}

/// What must be true for an item to be **enabled** — the Word-97 grey-out
/// contexts. This is context gating over seams that all exist, not phasing
/// (lock #4 governs *presence*; `Gate` governs the grey).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Always enabled.
    Always,
    /// Needs an open document.
    Doc,
    /// Needs a non-empty selection (Cut/Copy).
    Selection,
    /// Needs an available undo step.
    UndoStack,
    /// Needs an available redo step.
    RedoStack,
}

impl Gate {
    /// Whether the gate passes under `cx`.
    pub const fn enabled(self, cx: &MenuContext) -> bool {
        match self {
            Self::Always => true,
            Self::Doc => cx.has_doc,
            Self::Selection => cx.has_selection,
            Self::UndoStack => cx.can_undo,
            Self::RedoStack => cx.can_redo,
        }
    }
}

/// The surface-state snapshot the menu bar + toolbar render from: enablement
/// facts and the toggle/zoom read-backs. Built by `EditorSurface::menu_context`
/// each frame — the bars never reach into the surface directly.
// `struct_excessive_bools`: this IS a bag of independent per-frame facts (five
// unrelated enablement/toggle read-backs), not a disguised state machine — an
// enum would misrepresent flags that vary independently.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct MenuContext {
    /// A document is open.
    pub has_doc: bool,
    /// Some caret holds a non-empty selection.
    pub has_selection: bool,
    /// An undo step is available.
    pub can_undo: bool,
    /// A redo step is available.
    pub can_redo: bool,
    /// The project-tree side panel is shown (View → Project Tree checkmark).
    pub tree_shown: bool,
    /// The integrated terminal dock is shown (View → Terminal checkmark, EDITOR-10).
    pub terminal_shown: bool,
    /// Soft-wrap is on for the open document (View → Soft-Wrap checkmark).
    pub wrap_on: bool,
    /// The open document's view zoom in percent, or `None` with no document
    /// (the toolbar then omits the zoom dropdown — nothing to zoom).
    pub zoom_percent: Option<u16>,
    /// The primary caret line's markdown heading level (0 = Normal body text,
    /// 1-6 = a `#`-heading), or `None` with no document — the Format strip Style
    /// dropdown's current-selection read-back.
    pub heading_level: Option<u8>,
}

/// One menu item: its label, its (existing, real) shortcut hint, the action it
/// dispatches, its enablement gate, and whether a Word-style group separator
/// precedes it.
pub struct MenuItem {
    /// The visible label.
    pub label: &'static str,
    /// The right-aligned shortcut hint — only chords that exist today
    /// (`""` = none). Display-only; the bindings live in the widget/panel.
    pub shortcut: &'static str,
    /// The dispatched action.
    pub action: MenuAction,
    /// The enablement gate (the Word-97 grey-out context).
    pub gate: Gate,
    /// Draw a separator above this item (Word's menu groups).
    pub sep_before: bool,
}

impl MenuItem {
    /// Shorthand constructor for the static tables below.
    const fn new(
        label: &'static str,
        shortcut: &'static str,
        action: MenuAction,
        gate: Gate,
        sep_before: bool,
    ) -> Self {
        Self {
            label,
            shortcut,
            action,
            gate,
            sep_before,
        }
    }

    /// The toggle read-back for check-style items (View's toggles), or `None`
    /// for plain command items.
    pub const fn checked(&self, cx: &MenuContext) -> Option<bool> {
        match self.action {
            MenuAction::ToggleTree => Some(cx.tree_shown),
            MenuAction::ToggleTerminal => Some(cx.terminal_shown),
            MenuAction::ToggleWrap => Some(cx.wrap_on),
            _ => None,
        }
    }
}

/// The File menu (Word-97 order, minus the unlanded Print group — EDTB-5).
const FILE_ITEMS: [MenuItem; 6] = [
    MenuItem::new("New", "", MenuAction::NewScratch, Gate::Always, false),
    MenuItem::new(
        "Open\u{2026}",
        "Ctrl+P",
        MenuAction::OpenFinder,
        Gate::Always,
        false,
    ),
    MenuItem::new(
        "Open Folder",
        "",
        MenuAction::OpenFolderCwd,
        Gate::Always,
        false,
    ),
    MenuItem::new("Save", "", MenuAction::Save, Gate::Doc, true),
    MenuItem::new("Save As\u{2026}", "", MenuAction::SaveAs, Gate::Doc, false),
    MenuItem::new("Close", "", MenuAction::CloseDoc, Gate::Doc, true),
];

/// The Edit menu (Word-97 order; Find… omitted until EDITOR-8 lands in-buffer
/// find — no honest target today).
const EDIT_ITEMS: [MenuItem; 6] = [
    MenuItem::new("Undo", "Ctrl+Z", MenuAction::Undo, Gate::UndoStack, false),
    MenuItem::new("Redo", "Ctrl+Y", MenuAction::Redo, Gate::RedoStack, false),
    MenuItem::new("Cut", "Ctrl+X", MenuAction::Cut, Gate::Selection, true),
    MenuItem::new("Copy", "Ctrl+C", MenuAction::Copy, Gate::Selection, false),
    MenuItem::new("Paste", "Ctrl+V", MenuAction::Paste, Gate::Doc, false),
    MenuItem::new(
        "Select All",
        "Ctrl+A",
        MenuAction::SelectAll,
        Gate::Doc,
        true,
    ),
];

/// The View menu — the real toggles (Preview/bars arrive in later phases).
const VIEW_ITEMS: [MenuItem; 3] = [
    MenuItem::new(
        "Project Tree",
        "",
        MenuAction::ToggleTree,
        Gate::Always,
        false,
    ),
    MenuItem::new(
        "Terminal",
        "Ctrl+`",
        MenuAction::ToggleTerminal,
        Gate::Always,
        false,
    ),
    MenuItem::new("Soft-Wrap", "", MenuAction::ToggleWrap, Gate::Doc, false),
];

/// The Insert menu (EDTB-3) — Table… raises the Word grid-picker.
const INSERT_ITEMS: [MenuItem; 1] = [MenuItem::new(
    "Table\u{2026}",
    "",
    MenuAction::InsertTablePicker,
    Gate::Doc,
    false,
)];

/// The Format menu (EDTB-3) — the menu twin of the Formatting strip, in Word's
/// grouping: character wraps, then lists, then indent, then the paragraph
/// (heading) style. Every item needs an open document (`Gate::Doc`); they act on
/// the caret's line/selection even with nothing selected, so no `Selection` gate.
const FORMAT_ITEMS: [MenuItem; 15] = [
    MenuItem::new(
        "Bold",
        "",
        MenuAction::Wrap(WrapMarker::Bold),
        Gate::Doc,
        false,
    ),
    MenuItem::new(
        "Italic",
        "",
        MenuAction::Wrap(WrapMarker::Italic),
        Gate::Doc,
        false,
    ),
    MenuItem::new(
        "Underline",
        "",
        MenuAction::Wrap(WrapMarker::Underline),
        Gate::Doc,
        false,
    ),
    MenuItem::new(
        "Strikethrough",
        "",
        MenuAction::Wrap(WrapMarker::Strike),
        Gate::Doc,
        false,
    ),
    MenuItem::new(
        "Bullet List",
        "",
        MenuAction::List(ListStyle::Bullet),
        Gate::Doc,
        true,
    ),
    MenuItem::new(
        "Numbered List",
        "",
        MenuAction::List(ListStyle::Numbered),
        Gate::Doc,
        false,
    ),
    MenuItem::new(
        "Increase Indent",
        "",
        MenuAction::Indent(1),
        Gate::Doc,
        true,
    ),
    MenuItem::new(
        "Decrease Indent",
        "",
        MenuAction::Indent(-1),
        Gate::Doc,
        false,
    ),
    MenuItem::new("Normal Text", "", MenuAction::Heading(0), Gate::Doc, true),
    MenuItem::new("Heading 1", "", MenuAction::Heading(1), Gate::Doc, false),
    MenuItem::new("Heading 2", "", MenuAction::Heading(2), Gate::Doc, false),
    MenuItem::new("Heading 3", "", MenuAction::Heading(3), Gate::Doc, false),
    MenuItem::new("Heading 4", "", MenuAction::Heading(4), Gate::Doc, false),
    MenuItem::new("Heading 5", "", MenuAction::Heading(5), Gate::Doc, false),
    MenuItem::new("Heading 6", "", MenuAction::Heading(6), Gate::Doc, false),
];

/// The Tools menu (spelling is EDTB-6).
const TOOLS_ITEMS: [MenuItem; 1] = [MenuItem::new(
    "Command Palette\u{2026}",
    "Ctrl+Shift+P",
    MenuAction::CommandPalette,
    Gate::Always,
    false,
)];

/// The Help menu.
const HELP_ITEMS: [MenuItem; 1] = [MenuItem::new(
    "About the Editor",
    "",
    MenuAction::About,
    Gate::Always,
    false,
)];

/// The whole menu bar as data: `(title, items)` in Word-97 order. Insert +
/// Format land in EDTB-3; the standalone **Table** menu (cell/row ops) is still
/// absent — it has no landed backend (lock #4), and [`tests`] assert both the
/// omission and that no present menu is empty.
pub const MENUS: [(&str, &[MenuItem]); 7] = [
    ("File", &FILE_ITEMS),
    ("Edit", &EDIT_ITEMS),
    ("View", &VIEW_ITEMS),
    ("Insert", &INSERT_ITEMS),
    ("Format", &FORMAT_ITEMS),
    ("Tools", &TOOLS_ITEMS),
    ("Help", &HELP_ITEMS),
];

/// Render the menu bar and return the action the operator picked this frame, if
/// any. One generic loop over [`MENUS`]: plain items render as buttons with
/// their shortcut hint, toggle items as checked labels; gated items render
/// disabled (egui dims them through the shared theme).
pub fn show(ui: &mut Ui, cx: &MenuContext) -> Option<MenuAction> {
    let mut action = None;
    egui::menu::bar(ui, |ui| {
        ui.add_space(Style::SP_XS);
        for (title, items) in MENUS {
            ui.menu_button(title, |ui| {
                ui.set_min_width(MENU_MIN_W);
                for item in items {
                    if item.sep_before {
                        ui.separator();
                    }
                    let enabled = item.gate.enabled(cx);
                    let clicked = if let Some(on) = item.checked(cx) {
                        ui.add_enabled(enabled, egui::SelectableLabel::new(on, item.label))
                            .clicked()
                    } else {
                        ui.add_enabled(
                            enabled,
                            Button::new(item.label).shortcut_text(item.shortcut),
                        )
                        .clicked()
                    };
                    if clicked {
                        action = Some(item.action);
                        ui.close_menu();
                    }
                }
            });
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::{Gate, MenuAction, MenuContext, MENUS};

    /// A context with everything available (open doc, selection, both stacks).
    const fn full_context() -> MenuContext {
        MenuContext {
            has_doc: true,
            has_selection: true,
            can_undo: true,
            can_redo: true,
            tree_shown: true,
            terminal_shown: true,
            wrap_on: false,
            zoom_percent: Some(100),
            heading_level: Some(0),
        }
    }

    /// A fresh-surface context: no document at all.
    const fn empty_context() -> MenuContext {
        MenuContext {
            has_doc: false,
            has_selection: false,
            can_undo: false,
            can_redo: false,
            tree_shown: false,
            terminal_shown: false,
            wrap_on: false,
            zoom_percent: None,
            heading_level: None,
        }
    }

    #[test]
    fn every_menu_is_nonempty_and_every_item_is_labeled() {
        // Lock #4: a menu that would be empty is omitted entirely, so every
        // menu that IS present must have at least one real item.
        for (title, items) in MENUS {
            assert!(!items.is_empty(), "menu {title} shipped empty");
            for item in items {
                assert!(!item.label.is_empty(), "{title} has an unlabeled item");
            }
        }
    }

    #[test]
    fn the_standalone_table_menu_is_still_omitted() {
        // Insert + Format land in EDTB-3 (asserted present below); the standalone
        // Table menu (cell/row ops) has no landed backend, so it must be absent
        // (lock #4), not present-but-empty.
        let titles: Vec<&str> = MENUS.iter().map(|(t, _)| *t).collect();
        assert!(
            !titles.contains(&"Table"),
            "the standalone Table menu shipped before its backend"
        );
        assert!(titles.contains(&"Insert"), "Insert lands in EDTB-3");
        assert!(titles.contains(&"Format"), "Format lands in EDTB-3");
        assert_eq!(MENUS.len(), 7, "the seven real menus ship through EDTB-3");
    }

    #[test]
    fn insert_and_format_drive_the_md_actions_engine() {
        // EDTB-3 §7 — every Insert/Format item routes to a real engine action
        // (no dead entries), and each is doc-gated (grey with no document).
        let insert: Vec<MenuAction> = MENUS
            .iter()
            .find(|(t, _)| *t == "Insert")
            .expect("Insert menu")
            .1
            .iter()
            .map(|i| i.action)
            .collect();
        assert_eq!(insert, vec![MenuAction::InsertTablePicker]);

        let format = MENUS
            .iter()
            .find(|(t, _)| *t == "Format")
            .expect("Format")
            .1;
        for item in format {
            assert!(
                matches!(
                    item.action,
                    MenuAction::Wrap(_)
                        | MenuAction::List(_)
                        | MenuAction::Indent(_)
                        | MenuAction::Heading(_)
                ),
                "{} is not a formatting action",
                item.label
            );
            assert!(
                matches!(item.gate, super::Gate::Doc),
                "{} should be document-gated",
                item.label
            );
        }
        // The strip's whole vocabulary is present: B/I/U/S, both lists, both
        // indents, and Normal + all six heading levels.
        assert!(format
            .iter()
            .any(|i| i.action == MenuAction::Wrap(super::WrapMarker::Bold)));
        for lvl in 0..=6 {
            assert!(
                format.iter().any(|i| i.action == MenuAction::Heading(lvl)),
                "Heading {lvl} missing from the Format menu"
            );
        }
    }

    #[test]
    fn find_is_omitted_until_editor_8_lands() {
        // In-buffer find (EDITOR-8) has not landed; an Edit → Find… entry would
        // be a dead item (the file finder is not text find).
        for (_, items) in MENUS {
            for item in items {
                assert!(
                    !item.label.to_lowercase().starts_with("find"),
                    "Find shipped without the EDITOR-8 backend"
                );
            }
        }
    }

    #[test]
    fn word_97_menu_order_and_file_edit_contents() {
        let titles: Vec<&str> = MENUS.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            titles,
            vec!["File", "Edit", "View", "Insert", "Format", "Tools", "Help"],
            "the Word-97 menu order (minus the still-omitted Table menu)"
        );
        let file: Vec<&str> = MENUS[0].1.iter().map(|i| i.label).collect();
        assert_eq!(
            file,
            vec![
                "New",
                "Open\u{2026}",
                "Open Folder",
                "Save",
                "Save As\u{2026}",
                "Close"
            ]
        );
        let edit: Vec<&str> = MENUS[1].1.iter().map(|i| i.label).collect();
        assert_eq!(
            edit,
            vec!["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]
        );
    }

    #[test]
    fn gates_grey_out_by_context_not_by_missing_backend() {
        let full = full_context();
        let empty = empty_context();
        // Everything is enabled when the context provides it…
        for (_, items) in MENUS {
            for item in items {
                assert!(
                    item.gate.enabled(&full),
                    "{} disabled under a full context",
                    item.label
                );
            }
        }
        // …and only the always-available commands stay enabled with no doc.
        for (_, items) in MENUS {
            for item in items {
                let expect = matches!(item.gate, Gate::Always);
                assert_eq!(
                    item.gate.enabled(&empty),
                    expect,
                    "{} enablement wrong with no document",
                    item.label
                );
            }
        }
    }

    #[test]
    fn toggles_read_back_their_state_and_commands_do_not() {
        let cx = full_context();
        let mut toggles = 0;
        for (_, items) in MENUS {
            for item in items {
                match item.action {
                    // Project Tree + Terminal both read back `true` in `full_context`.
                    MenuAction::ToggleTree | MenuAction::ToggleTerminal => {
                        assert_eq!(item.checked(&cx), Some(true));
                        toggles += 1;
                    }
                    MenuAction::ToggleWrap => {
                        assert_eq!(item.checked(&cx), Some(false));
                        toggles += 1;
                    }
                    _ => assert_eq!(item.checked(&cx), None, "{} checked?", item.label),
                }
            }
        }
        assert_eq!(toggles, 3, "the three View toggles are check-style");
    }
}
