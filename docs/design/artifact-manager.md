# AFM — Artifact Manager interaction-layer completion

**Epic prefix:** `AFM`
**Status:** locked 2026-06-15 (operator handed the canonical Claude-Design bundle
`mde-4-workgroups-desktop` → `Artifact Manager.html` + `files-app.jsx`)
**Surface:** `crates/services/mde-files` (libcosmic GUI, window title "Artifact Manager")
**Design source:** the exported prototype (React/HTML) — recreate its **structure +
behavior**, render through the existing **IBM Carbon** tokens (operator decision,
2026-06-15: *"Keep Carbon, build structure"*).

## Why

The operator opened the live Artifact Manager and reported the interaction layer is
broadly dead: *"Most of the interface does not work … I believe it is stubbed out."*
Concretely: window controls (min/max/close), the sidebar panel-toggle + self row,
read-only breadcrumbs, no local file can be acted on, Inbox shows the home directory,
Outbox does nothing, the grid/list toggle is inert on most views, and the left column
never populated with peers. The prototype is the canonical spec for what each of those
should do.

## Locked decisions

| # | Fork | Decision |
|---|------|----------|
| 1 | Visual look | **Carbon, not the prototype's PatternFly/Red Hat warm-dark.** The prototype's amber/rust + Red Hat surfaces stay re-sourced to the IBM Carbon ramp in `mde-files/theme.rs` (§4 lock + `palette_is_carbon_gray_100_dark` regression test intact). Build the *layout + behavior* only. |
| 2 | Scope | The **structure/behavior** of every prototype surface (titlebar, sidebar, toolbar, mesh overview, peer folder, inbox, outbox, downloads, local veil) — i.e. make the existing ported views actually *do* what the prototype does. |
| 3 | Data source | Unchanged from the live fixes already landed: peers/self/overlay from the replicated **directory** (`DirectoryService::build_directory`) over the Bus, not the empty SQLite `nodes` table; the GUI reads the system bus via `mde_bus::client_data_dir()`. |

## Gap analysis (current code → prototype intent)

| Area | Current state | Target |
|------|---------------|--------|
| Window controls | `TitlebarMinimize/Maximize/Close` are no-op match arms (`app.rs`) | Wire to `cosmic::iced::window` (`minimize`/`toggle_maximize`/`close` on `window::latest()`), mirroring `mde-workbench::dispatch_window_action`. |
| Sidebar panel-toggle | `icons::PANEL_RIGHT` → `Message::Noop` | Collapse/expand the sidebar (`sidebar_collapsed` state; the prototype's `.sidebar-collapsed` 0px grid). |
| Self row | `Message::Noop` | Open this node's shared folder (a `Peer`-style self view). |
| "+ Peer" footer | `Message::Noop` | Route to peer registration / enroll. |
| Breadcrumbs | rendered as static `text(...)` | Each crumb is a button → navigate (Mesh→Overview, Home→MeshHome, peer→peer, mid-segments→`MeshFolderPop`). |
| Local file actions | rows only select | Double-click / Enter opens (descend folder, else `xdg-open`); right-click context menu reachable. |
| Inbox | `list("")` falls back to `local.list("")` → shows `$HOME` | Inbox shows only the mesh inbox; honest empty state when no Bus inbox (never `$HOME`). |
| Outbox | sidebar row → `Noop`; no `View::Outbox` | New `View::Outbox` + backend source (sent-file log) + view + sidebar wiring. |
| Grid/List toggle | only `peer_folder` honors `layout` | Thread `layout` into inbox/downloads/local/mesh-home + render grid (object cards) vs list everywhere. |
| Left-column peers | renders `snap.peers` (was empty) | Data already fixed (directory-sourced); verify populates; relabel stale "tailnet" → "overlay". |
| PrimaryAction / PeerCardSend | `Noop` | Wire Send / Share / New + per-card Send-To. |

## Acceptance (runtime-observable, §7)

1. Titlebar min/max/close actually minimize, toggle-maximize, and close the window.
2. The sidebar panel-toggle collapses + restores the sidebar; the self row opens this
   node's shared folder; "+ Peer" reaches registration.
3. Every breadcrumb segment is clickable and navigates; the trailing MESH/LOCAL tag is
   correct per view.
4. In the Local browser a folder row opens on activate (descends) and a file row opens
   its default app; right-click shows the context menu.
5. Inbox shows the mesh inbox or an honest empty state — never the home directory.
6. Outbox is a real view listing files this node has sent (or an honest empty state).
7. The grid/list toggle visibly changes the layout on every file-listing view.
8. The sidebar + overview populate with the live peer roster (directory-sourced); no
   "0 of 0 peers" on a populated mesh; no stale "tailnet" copy.
9. `cargo test -p mde-files` green; the §4 hex lint + `palette_is_carbon_gray_100_dark`
   stay green (no warm-dark drift).

## Out of scope

- Reverting the §4 Carbon lock to the prototype's warm-dark palette (rejected, Q1).
- The desktop shell / terminal peek / bottom panel from the bundle's other files
  (Cosmic owns the desktop — §6; only the Artifact Manager app is in scope).
- SMB "Network" + "Cloud Files" deepening beyond what already ships (the prototype has
  neither; tracked separately if the operator wants them advanced).
