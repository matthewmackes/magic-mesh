# NAVBAR — bottom nav taskbar (relocate the shell dock, Carbon icon-first)

Operator-locked 2026-07-03 (15-question `/plan` survey). Moves the shell's surface
launcher from the **left vertical rail** (`mde-shell-egui/src/dock.rs`, a `SidePanel::left`)
to a **full-width horizontal taskbar pinned to the bottom edge**, redrawn Carbon
icon-first, and **merges the separate status chrome** (`chrome.rs`) into the same bar.
Supersedes the left-rail layout of E12-3b; complements — does not replace — the locked
compact↔expand model in [`workbench-control-surface.md`](workbench-control-surface.md).

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Bar model | **Full-width taskbar** — fixed height, spans the entire bottom edge (Windows/Carbon-console idiom). Not a centered floating dock. |
| 2 | Status integration | **Unified bottom bar** — surface glyphs (left/center) + the status tray (clock, peer count, health, quick-toggles) on the right, in ONE bar. The separate status chrome is folded in. |
| 3 | Surface organisation | **Grouped with dividers** — three sections separated by thin Carbon dividers: **mesh-control** (Workbench, Mesh Map, Instances, Desktop) ∣ **apps** (Music, Media, Files, Voice, Browser, Terminal, Chat, Editor) ∣ **system** (System, Storage, About). |
| 4 | Left rail fate | **Removed** — the bottom bar fully replaces the left rail; one nav, no duplication. |
| 5 | Labels | **Icon + tiny always-on label** — each glyph carries a small Carbon caption beneath it, always visible (icon-first, but named). |
| 6 | Size | **Standard 48px bar** (~24px glyphs) — balanced density for ~15 grouped glyphs + tray. |
| 7 | Active mark | **Accent top-border + tint** — a Carbon accent line along the top edge of the active glyph's cell, plus a brighter glyph tint. |
| 8 | Material | **Flat Carbon, edge-to-edge** — solid `layer`/`surface` token fill, a hairline top divider; no gloss/float. |
| 9 | Glyph tint | **Two-tone** — the active surface's glyph renders **filled/solid**, inactive glyphs render **outline-only**; both from `mde-theme` tokens (no raw hex). |
| 10 | Hover | **One-line hint + cell highlight** — hover reveals the surface's existing `hint` text as a tooltip and softly highlights the cell. |
| 11 | Badges | **Live indicators** — Chat unread badge (the ONE notification surface), Mesh Map peer count, System/node health-tinted dot; drawn on the glyph cells. |
| 12 | Narrow fit | **Overflow to a 'More' (⋯) tray** — when width can't fit all glyphs + tray, the least-used surfaces collapse into a `⋯` button that opens a small tray. |
| 13 | Order | **Tray-style** — apps pack from the left; the **system** group (System/Storage/About) is pushed hard right, next to the status tray. |
| 14 | Keyboard + menu | **Full** — `Super`+1…9/0 jump to a surface (reuse `hotkeys.rs`), arrow-key nav when the bar is focused, right-click a glyph for a context menu (pin, info, close). |
| 15 | Density | **Tie to compact↔expand** — the bar honours the locked workbench mode: **expand** = the 48px labeled bar; **compact** = a denser, icon-only (labels hidden) version. Reuses the control-surface mode enum. |

## Architecture

The dock stays "pure chrome" (reads + writes the active [`Surface`]); only its
**placement, layout axis, look, and the folded-in status tray** change.

- **Placement / layout axis** (`dock.rs` + `main.rs`): replace the
  `egui::SidePanel::left("shell-dock")` mount with an
  `egui::TopBottomPanel::bottom("shell-taskbar")` at a fixed 48px (expand) height;
  lay the entries out **horizontally** (`ui.horizontal`) instead of vertically. The
  surface **body** now fills the `CentralPanel` above the bar. The left rail is
  deleted, not toggled.
- **Grouping + order** (`dock.rs`): the `ALL` table is partitioned into the three
  groups; render group A (mesh-control) then a `divider()`, group B (apps) then a
  `divider()`, then **flexible space**, then the status tray, then group C (system)
  hard-right (tray-style, #13). A small `group_of(Surface) -> Group` classifier keeps
  the partition declarative.
- **Glyph cell** (`dock.rs`): each cell = the `brand::icons` Carbon glyph (filled when
  active, outline when idle — #9) + the tiny label (#5), an accent top-border strip +
  brighter tint when active (#7), a hover highlight + `hint` tooltip (#10), and any
  live badge (#11). All paint through `mde-egui::Style` tokens (§4).
- **Unified status tray** (`chrome.rs` → the bar): the status widgets currently drawn
  by `chrome.rs` (clock, peer count, health, quick-toggles, role badge, version tag)
  move into a right-aligned tray segment of the taskbar. `chrome.rs` is retired to a
  tray-widget module (or its top strip removed); the shell no longer draws a separate
  status strip.
- **Badges** (`dock.rs`): a `badge_for(Surface, &ShellState) -> Option<Badge>` reads
  the already-available state — Chat unread from `chat::total_unread`, peer count from
  the mesh mirror, node health from the host/system mirror — and paints a small Carbon
  count/dot on the cell.
- **Keyboard + context menu** (`hotkeys.rs` + `dock.rs`): `Super`+N already exists in
  `hotkeys.rs` — bind it to the visible order; add arrow-key focus traversal and a
  `response.context_menu(...)` per cell (pin, info→About-of-surface, close-if-closable).
- **Overflow tray** (`dock.rs`): measure available width; if the grouped glyphs + tray
  overflow, move trailing/low-priority surfaces into a `⋯` popup (`egui::menu`).
- **Compact↔expand tie** (`main.rs` + `dock.rs`): the existing compact/expand mode
  (control-surface enum) selects the bar variant — expand = 48px + labels; compact =
  a shorter icon-only bar. One mode drives the whole shell.

## Acceptance (runtime-observable, per task)
- The shell boots with the launcher as a **full-width bar on the bottom edge**; the
  **left rail is gone**; the surface body fills the space above it.
- The bar shows the three groups **mesh-control ∣ apps ∣ system** with Carbon dividers;
  system/Storage/About sit hard-right beside the status tray (clock/peers/health).
- Each glyph shows its Carbon icon + a tiny label; the **active** surface's glyph is
  **filled** with an **accent top-border**, idle glyphs are outline-only; hover shows
  the hint tooltip + highlights the cell.
- Clicking a glyph switches the surface; `Super`+N jumps to the Nth; arrow keys move
  focus; right-click opens the context menu.
- Chat shows an unread badge when messages are pending; Mesh Map shows the live peer
  count; a degraded node shows a health-tinted dot.
- Narrowing the window collapses the least-used surfaces into a `⋯` tray (all still
  reachable); no glyph is silently dropped.
- Switching the shell to **compact** mode shrinks the bar to icon-only; **expand**
  restores the 48px labeled bar.
- All colours/metrics come from `mde-theme`/`mde-egui` tokens (§4 — no raw hex); the
  bar builds + tests green and renders (§7).

## Risks
- **`chrome.rs` fold-in is the biggest change** — the status widgets are 1040 lines;
  moving them into the bar's tray without regressing the clock/peers/health/toggles is
  the real work. Mitigate: move widget-by-widget, keep the same token styling.
- **Width budget** — 15 grouped glyphs + labels + a status tray is tight on a small
  panel; the overflow tray (#12) + compact mode (#15) are the pressure valves. Measure
  before painting.
- **Interlocking files** — `dock.rs`, `chrome.rs`, `main.rs` all move together; the
  tasks below largely **serialise on these files** (one worker sequence, not parallel).
- **EDITOR-1 just added `Surface::Editor`** — the bar must include it in the apps group
  (15 surfaces, not 14).

## Out of scope
- A user-draggable/reorderable/pinnable bar as the default (#13 chose tray-style order,
  not user-reorder); pinning rides only in the context menu (#14) as a light touch.
- A second (top) bar — the design is one unified bottom bar.
- Auto-hide / reveal-on-hover behaviour (fixed bar; revisit if a full-screen surface
  needs it).
- Per-user themes/skins for the bar (single platform brand, QBRAND).

## Nav Bar ⇄ Chooser union — "one picker" (operator 2026-07-03, follow-up survey)

The operator locked a **full merge**: the bar and the Desktop **Chooser** (`chooser.rs`,
the "Picker" — a grid of discovered display sources: mesh peers, LAN, local VMs, the
TESTVM endpoints) become **one picker system** with two faces, sharing one
`ChooserState`. The Nav Bar picks *surfaces* (what) and *sources* (which remote desktop)
in one place; the standalone chooser becomes the picker's expanded view.

| # | Decision | Lock |
|---|----------|------|
| U1 | Degree of union | **Full merge into one picker** — surfaces + desktop sources + live sessions in one bar-anchored model; the chooser surface is its expanded face, not a separate island. |
| U2 | Desktop entry | **Split button** — main click reconnects the last/active remote desktop (opens the picker if none); the caret opens a **source flyout** (the chooser's discovered+pinned sources as a bar popup). |
| U3 | Live sessions | **Temporary bar entries** — each connected remote desktop shows as its own live glyph in the bar (taskbar running-window model), appearing on connect, removed on disconnect; click focuses that session. |
| U4 | Two faces, one state | The bar flyout (compact) and the `chooser.rs` surface (expanded) **share one `ChooserState`** — single source of truth for sources/pins/sessions; the compact↔expand mode (#15) selects which face shows. |

**Mechanism:** `chooser.rs` already owns `ChooserState` (sources, `ChooserPrefs` pins,
`take_connect`, `poll`, thumbnails) + `chooser_grid`/`connect_picker`. The union lifts a
**compact projection** of that state into the bar: (a) the Desktop cell renders as a
split button reading `ChooserState` for the last/active target + a caret popup that
reuses `connect_picker`/a slim `chooser_grid`; (b) a `sessions()` view of active VDI
connections (from the `vdi`/chooser session state) drives the temporary bar entries; (c)
the full `chooser_panel` stays as the expanded surface, now reading the *same* state the
bar mutates — no second store. Reuses the NAVBAR-5 badge + NAVBAR-7 flyout/overflow +
NAVBAR-8 compact↔expand machinery (§6: glue, not a second picker).

**Sequencing:** these ride **after** NAVBAR-1..3 (the bar must exist first) and touch
`dock.rs` + `chooser.rs` + `main.rs` — serialise with the other NAVBAR tasks on those
files.

## Tasks → see `docs/WORKLIST.md` NAVBAR-1..8 + NAVBAR-U1..U4.
