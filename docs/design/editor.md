# EDITOR — native Zed-style code editor surface (`mde-editor-egui`)

Operator-locked 2026-07-03 (2-round `/plan` survey). A native code editor surface for
the DRM-native egui platform, inspired by [Zed](https://zed.dev) — a fast, keyboard-driven,
Rust code editor — adapted to the platform's egui shell + mesh. Mounts as a dock surface
(`Surface::Editor`) alongside Files/Terminal/etc., following the embedded-lib-panel idiom.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Text core | **Custom rope-backed egui widget** — a `ropey`-backed buffer + a hand-built egui text widget (line layout, gutter, cursor, selection, multi-cursor), the Zed-grade architecture in egui. Not egui's `TextEdit` (too limited for code). |
| 2 | Syntax highlighting | **Tree-sitter** — incremental per-language parse trees (what Zed uses); enables structural selection, folding, symbols. Pure-Rust grammars, farm-vendorable. |
| 3 | Language intelligence (LSP) | **Follow-on phase** — ship the solo editor first (buffer + tree-sitter + files + find), add the LSP client (diagnostics/completion/goto/hover/rename) as **Phase 2**. |
| 4 | Collaboration | **Mesh-native CRDT co-editing as its own phase** — Zed's multiplayer, but **P2P over the mesh** (mde-bus/Nebula, no cloud/account). **Phase 3**, reusing the mesh-sync seam other surfaces use. |
| 5 | Keymap / modality | **Standard keymap only** (conventional Ctrl-based) — no Vim mode. |
| 6 | Layout | **Tabs + splittable panes** — tabbed buffers with horizontal/vertical splits (side-by-side compare), **reusing `mde-term-egui`'s split-pane machinery** (TERM-4/5). |
| 7 | Navigation | **Full** — fuzzy file finder (Cmd-P), command palette (Cmd-Shift-P), and project-wide search (ripgrep-style), reusing the shell's existing search/ladder patterns. |
| 8 | Platform integration | **Reuse Files + Terminal, own project tree** — the editor has its own lightweight project panel; reuses `mde-files` "Send-to-Editor" to open files; embeds `mde-term-egui` as the integrated terminal (§6: glue, not reimplementation). |

## Architecture

**New crate `crates/desktop/mde-editor-egui`** exposing an `editor_panel(ctx, ...)` seam +
`EditorSurface` (mirroring `mde_files_egui::files_panel`/`real_browser`), mounted in
`mde-shell-egui/src/dock.rs` as `Surface::Editor` (enum + `ALL`/`SURFACES` + label + render
arm), with a new `IconId::Editor` glyph (`assets/brand/construct/surface-editor.svg` + the
`brand::icons` registry).

- **Buffer core** (`ropey`): open/save (encoding-aware), an efficient line index, undo/redo
  (grouped edits), large-file tolerance. One `Buffer` per open document; buffers are shared
  across panes (a split shows the same buffer).
- **Text widget** (egui): custom paint of visible lines + gutter (line numbers, diagnostics
  later), block/bar cursor, single + **multi-cursor** + column selection, mouse (click/drag/
  double/triple-select) + keyboard editing, soft-wrap toggle, horizontal + vertical scroll.
  Fonts via the shell's existing mono font stack (reuse `mde-term-egui::fonts` idiom).
- **Highlighting** (tree-sitter): incremental re-parse on edit; a grammar set (rust, python,
  js/ts, toml, json, markdown, bash, C/C++, go, yaml); highlight → Carbon token colors via
  `mde-theme` (§4, a code-theme token module). Structural features (folding, expand-selection,
  symbol outline) ride the same tree.
- **Panes** (reuse TERM-4/5 split machinery): a pane tree with tabbed buffers per pane +
  horizontal/vertical splits; a shared tab/close/focus model.
- **Navigation:** fuzzy file finder + command palette + project-search reuse the shell's
  search/ladder store; project search shells `ripgrep` (already a build-farm tool) honestly
  (fallback message if absent).
- **Integration (§6):** own project-tree panel; `mde-files` gains a "Send-to-Editor" action
  (reuse the existing surface-launch/Bus path the other Send-to actions use); `mde-term-egui`
  embedded as an integrated terminal panel (reuse its `TabbedTerminal`, like TERM-16 mounted it).

## Phases

**Phase 1 — the solo code editor** (EDITOR-1..12): the surface, buffer, text widget,
multi-cursor, tree-sitter highlighting, tabs+splits, finder+palette+search, project tree,
Files/Terminal integration, save/reload, folding+symbols. Ships as a complete, reachable,
dock-mounted editor.

**Phase 2 — LSP** (EDITOR-LSP-1..N): an LSP client subsystem — server lifecycle per language,
diagnostics (gutter + inline), completion, hover, goto-definition, find-references, rename,
signature help, formatting.

**Phase 3 — mesh-native collaboration** (EDITOR-COLLAB-1..N): a CRDT buffer, share-session
published over mde-bus/Nebula (P2P, no cloud), remote cursors/selections, follow mode,
per-session permissions, presence in the Mesh Map.

## Acceptance (Phase 1, runtime-observable)
- `Surface::Editor` is in the dock (its Carbon glyph renders), opens, and edits a real file
  on disk (open → type → save → the bytes change).
- Rope buffer handles a large file (e.g. 100k lines) without stalling; undo/redo works;
  multi-cursor edits apply to every cursor.
- Tree-sitter highlights rust/python/etc. correctly, re-highlighting incrementally on edit,
  through the Carbon code-theme tokens (no raw hex).
- Tabs + a horizontal/vertical split show two buffers side-by-side; focus/close work.
- Cmd-P fuzzy-opens a file by name; Cmd-Shift-P runs a command; project search finds a string
  across the tree (ripgrep) and jumps to the hit.
- The project tree lists the open folder; `mde-files` "Send-to-Editor" opens a file in the
  editor; the embedded terminal panel runs a real shell (mde-term-egui).
- `--version`/reachability: launched from the dock, it does real work (not a mockup).

## Risks
- **Custom text widget is substantial** — cursor/selection/multi-cursor/scroll/soft-wrap in
  egui is real work; the biggest cost. Mitigate by reusing mde-term-egui's text-layout idioms.
- **Tree-sitter grammar vendoring on the airgapped farm** — grammar crates must build offline
  (pure-Rust C-via-cc; confirm each vendors). Start with a core language set.
- **Split-pane reuse** — mde-term-egui's split machinery is terminal-shaped; adapting it to
  buffer panes may need a generalization (a shared `mde-panes` helper) rather than a copy.
- **ripgrep dependency** for project search — present on the farm/workstation; honest fallback.

## Out of scope (Phase 1)
- Vim mode (decision #5 — standard keymap only).
- LSP (Phase 2) and mesh collaboration (Phase 3).
- An extension/plugin marketplace (Zed has one; far-future, not planned).
- Debugger integration / notebook cells.

## Tasks → see `docs/WORKLIST.md` EDITOR-1..12 (Phase 1), + the EDITOR-LSP / EDITOR-COLLAB phase stubs.
