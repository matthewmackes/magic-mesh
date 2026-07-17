# TMUX-FC — first-class tmux control chrome in the platform terminal

Operator-locked 2026-07-04 (16-Q survey). Make **tmux a first-class citizen** of the
platform terminal (`mde-term-egui`) via extensive control chrome — iTerm2-style
**control mode** where tmux's windows/panes become the terminal's own native tabs/splits,
driven bidirectionally by rich GUI chrome, mesh-aware, persistent.

## Locked decisions (16)

| # | Area | Lock |
|---|------|------|
| 1 | Depth | **Control mode (`tmux -CC`)** — tmux windows→native tabs, panes→native splits; GUI ops issue tmux commands, tmux events update the GUI. The deepest integration. |
| 2 | Mapping | **Windows→tabs, panes→splits** (the existing TERM-5 tabs + TERM-4 splits render tmux's layout natively). |
| 3 | Coexistence | **tmux-backed tabs coexist with native** — a tab is either a native split-tree or a tmux-controlled session; both live in the one tab strip. |
| 4 | Chrome | **All of it** — a session/window/pane **sidebar tree** + a **native Quazar status bar** + a **toolbar + command palette** + **context menus** on tabs/panes. |
| 5 | Sessions | Full GUI ops: **create / attach / detach · rename / kill · list-all-incl-detached · session templates** ("projects"). |
| 6 | Persistence | **Auto-reattach on relaunch** — the terminal remembers attached sessions + reattaches on restart; detached sessions keep running on the node. |
| 7 | Mesh | **Attach to any node's tmux over the mesh** — the picker lists tmux sessions on ALL mesh nodes (via the TERM remote/roster SSH-over-overlay); control a peer's tmux with the same chrome. |
| 8 | Status bar | **Native Quazar status** (window tabs + session + clock), ignoring the user's tmux `status-*` config for a consistent look. |
| 9 | Pane ops | **All**: split/close/zoom · break/join/swap/move · **drag-resize + drag-reorder** (mapped to `resize-pane`/move) · rename window/pane titles. |
| 10 | Copy/scroll | **Native GUI scrollback + selection + search** (TERM's own), NOT tmux copy-mode; yank into tmux buffers + the mesh clipboard. |
| 11 | Layouts | **5 custom mesh-styled layout presets** (below), not the stock tmux five; mesh-synced. |
| 12 | Keys | **Both** — the tmux prefix (Ctrl-B/configured) still works inside panes AND native GUI chords (remappable). |
| 13 | Presets | **Mesh Ops · Node Watch · Cloud/OpenStack · Dev/Build · AI-CLI (Claude + Codex)** (see below). |
| 14 | Config | **Platform-managed tmux config** (a Quazar default, mesh-synced; no per-user file hand-editing). |
| 15 | Palette | **Curated common commands** (~30 most-used tmux actions), fuzzy-searchable. |
| 16 | Default | **Opt-in per tab** — plain shells by default; "New tmux session" opens a tmux-backed tab. |

## The 5 mesh-styled layout presets (#11/#13)
Each = a named tmux window/pane layout + seeded commands, in the Quazar style:
1. **Mesh Ops** — `meshctl status` · peers roll-up · mesh log follow · a control shell.
2. **Node Watch** — `btop` · `journalctl -f` · disk/SMART · a shell (per-node health).
3. **Cloud / OpenStack** — `openstack` ops · Heat/instances · service logs · a shell.
4. **Dev / Build** — an editor pane · a build/test shell · a run/logs pane · git.
5. **AI-CLI** — the **Claude CLI** + **Codex CLI** side by side + a work shell (+ logs).

## Architecture (mde-term-egui)

- **Control-mode core** (a new `tmux.rs`): spawn `tmux -CC` (attach/new), parse the
  control-mode protocol (`%output`, `%window-add/close/renamed`, `%layout-change`,
  `%session-changed`, `%begin/%end`, etc.); maintain a live model of sessions→windows→panes.
  Map each tmux window to a `tabs::TabbedTerminal` tab and each pane to a `splits`
  leaf, feeding pane output into the existing TERM-3 widget grid. GUI actions
  (split/close/rename/resize/select) emit the corresponding `tmux` command over the control
  channel; `%`-events reconcile the GUI. Reuse TERM-4 splits + TERM-5 tabs verbatim (§6 —
  glue, not a new multiplexer).
- **Chrome** (`panel.rs`/new `tmux_ui.rs`): the sidebar tree, the native Quazar status bar
  (replacing tmux's), a toolbar + the curated command palette (reuse the terminal's palette
  idiom), and context menus on tabs/panes — all issuing tmux commands.
- **Sessions + persistence**: create/attach/detach/kill/rename + a session picker listing
  ALL sessions (attached + detached, local + mesh); auto-reattach remembered sessions on
  relaunch (persisted in the platform-managed config). Session templates seed a layout +
  commands.
- **Mesh**: extend `remote.rs`/`roster.rs` (the SSH-over-overlay peer-session machinery
  TERM already has) to enumerate + attach `tmux -CC` on peer nodes — one chrome, any node.
- **Config**: a platform-managed Quazar `.tmux.conf` (mesh-synced with the other browser/
  shell settings), a GUI settings pane for the common knobs (prefix, mouse, history).
- **Keys**: the tmux prefix passes through to panes; native GUI chords (remappable via the
  terminal keymap) drive the GUI ops.

## Acceptance (runtime-observable; per task)
- Opening a tmux-backed tab attaches `tmux -CC`; its windows appear as native tabs and panes
  as native splits; splitting/closing/renaming/resizing in the GUI issues the tmux command
  and the tmux `%`-event updates the chrome.
- The sidebar tree + native status bar + toolbar/palette + context menus all drive tmux;
  create/attach/detach/kill/rename/list work; detached sessions survive + auto-reattach.
- The session picker lists + attaches tmux on any mesh node; the same chrome controls it.
- The 5 mesh-styled presets open their layouts + seeded commands; native scrollback + yank
  work; the tmux prefix + GUI chords both work.
- All via TERM's native tabs/splits/widget + the Quazar `Style` tokens (§4); no second
  multiplexer, no stubs.

## Risks
- **Control-mode protocol** — `tmux -CC` output parsing is fiddly (interleaved `%output`,
  layout strings, escaping); needs a robust incremental parser + tests against real tmux.
- **Reconciling GUI ↔ tmux** — every GUI op must round-trip through tmux (never mutate the
  native tree directly for a tmux tab), or the two models diverge.
- **Mesh attach latency** — control mode over SSH-over-overlay adds latency; keep the UI
  responsive (optimistic + reconcile).
- **Coexistence** — native-split tabs and tmux tabs in one strip must not confuse the split
  machinery (a tmux tab's splits are tmux-owned, not directly user-resizable outside tmux).

## Out of scope (v1)
- Respecting arbitrary user `.tmux.conf` status themes (#8 = native Quazar status).
- Non-tmux multiplexers (screen/zellij).
- A general tmux scripting IDE (the palette + templates cover the common set).

## Tasks → `docs/WORKLIST.md` TMUX-FC-1..8.
