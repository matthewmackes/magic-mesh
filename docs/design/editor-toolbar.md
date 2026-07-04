# EDTB — the Word-97 toolbar for the text editor (`mde-editor-egui`)

Operator-locked 2026-07-04 (10-Q `/plan` survey). An extensive toolbar chrome for the
editor surface matching the features/functions of **Microsoft Word 97** — the menu bar
+ Standard toolbar + Formatting toolbar — adapted honestly to a plain-text/markdown
editor on the Quazar platform. Companions: `editor.md` (the editor architecture;
EDITOR-1..4/7/9 landed, EDITOR-5 in flight).

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Formatting semantics | **Markdown-backed** — formatting buttons make real, visible markup edits: Bold → `**sel**`, Italic → `*sel*`, Style dropdown → `#`-heading levels, lists → line prefixes. The buffer stays plain text; tree-sitter's markdown grammar highlights it. (Underline: md has none — insert `<u>…</u>`, honest HTML-in-md.) |
| 2 | Bars | **All three** — the menu bar (File/Edit/View/Insert/Format/Tools/Table/Help) + the Standard toolbar (New/Open/Save/Print/Cut/Copy/Paste/Undo/Redo/Find/Zoom…) + the Formatting toolbar (Style/Font/Size/B/I/U/lists/indent). |
| 3 | Look | **Carbon-dark Word layout** — the exact Word-97 button order/grouping/dropdowns drawn in Carbon tokens + line-art glyphs (§4). Not the beveled-grey retro skin. |
| 4 | Function scope | **Everything real, phased** — §7 hard rule: no dead buttons. A control appears only when its backend lands (print/spell/preview are later phases); no disabled placeholders. |
| 5 | Print | **CUPS `lp`** — Print sends the buffer as formatted plain text (mono; optional line numbers); Print Preview shows the paginated text. PDF-pretty rendering is a possible later upgrade. |
| 6 | Spelling | **System hunspell** + dictionaries (an RPM `Requires`); red squiggly underlines in the widget + the F7 spell dialog. |
| 7 | Font + Size dropdowns | **Global shell setting** — they set the platform-wide font/size (persisted shell config, applied live), not a per-buffer view. (Zoom stays the editor-view scale control, separate per Word.) |
| 8 | Tables + lists | **Word grid-picker → markdown table** — Insert Table opens the rows×cols hover grid, inserts a md-table skeleton; Bullets/Numbering toggle `- ` / `1. ` prefixes on selected lines; Increase/Decrease Indent shifts list nesting. |
| 9 | Bar visibility | **Compact-aware** — expanded mode shows all three bars; compact mode collapses to the menu bar only. (No per-bar View toggles.) |
| 10 | Preview | **Split preview toggle** — View→Preview (+ toolbar button) opens a side-by-side rendered-markdown pane (headings/bold/lists/tables via Carbon text styles). |

## Architecture

New modules in `crates/desktop/mde-editor-egui`: `menu_bar.rs` (the 8 Word menus →
existing seams: file ops, clipboard, undo/redo, finder/palette, view toggles),
`toolbar.rs` (Standard + Formatting strips over shared button/dropdown widgets),
`md_actions.rs` (the pure markdown edit engine: wrap-selection, heading-set,
line-prefix toggles, indent shift, table skeleton — all `Buffer` edits, unit-testable),
`table_picker.rs` (the hover grid), later `print.rs` (lp pipe), `spell.rs` (hunspell
seam), `preview.rs` (split rendered pane). `panel.rs` mounts: menu bar → toolbars →
(tree | editor | preview). Every action routes through the same seams the palette
uses (`EditorSurface`/`Buffer`/widget clipboard), §6 glue. Menus/toolbars honor the
compact↔expand mode (#9). The Font/Size global setting persists via the shell's
config path + applies through the mde-egui font install (its own task — cross-crate).

## Phases (per lock #4 — a button ships only with its backend)
- **P1 (EDTB-1..4):** menu bar + Standard (minus Print/Spell) + Formatting bars, md
  actions, grid-picker tables, zoom, compact-aware; Font/Size global-setting wiring.
- **P2 (EDTB-5):** Print + Print Preview (CUPS).
- **P3 (EDTB-6):** hunspell spelling + squiggles + F7 dialog (RPM `Requires: hunspell`).
- **P4 (EDTB-7):** the split markdown preview.

## Acceptance (runtime-observable)
- The editor shows the Word-97 menu bar + two toolbars (Carbon-dark, Word order);
  compact mode leaves only the menu bar.
- B/I/U/Style/lists/indent make the real markup edits at every caret (multi-cursor
  aware); undo groups sanely; the grid picker inserts a working md table.
- New/Open/Save/SaveAs/Cut/Copy/Paste/Undo/Redo/Find/Zoom all drive the real seams.
- Font/Size dropdowns change the platform font/size persistently (visible everywhere).
- P2: Print produces paper via CUPS; Preview paginates. P3: misspellings squiggle +
  F7 walks them (hunspell present via RPM). P4: the split preview renders headings/
  bold/lists/tables live as you type.

## Risks
- **mde-editor-egui contention** — EDITOR-5 (widget.rs) is in flight; EDTB P1 lands in
  panel.rs + new modules → serialize EDTB behind EDITOR-5's landing.
- **hunspell** = a system/RPM dep on the airgapped farm (build-time header need is
  avoidable by shelling `hunspell -a` — decide in EDTB-6).
- **Global font setting** crosses crates (shell config + mde-egui fonts install) —
  keep it a small, honest, persisted setting; don't rebuild a settings framework.
- Word-97 breadth invites dead buttons — lock #4's phasing is the guard.

## Out of scope
- True rich text / .rtf; WYSIWYG editing (the preview is read-only rendering).
- Word's Drawing toolbar, Columns, mail-merge, macros, grammar check.
- Toolbar customization/drag (Word had it; fixed layout here).

## Tasks → `docs/WORKLIST.md` EDTB-1..7.
