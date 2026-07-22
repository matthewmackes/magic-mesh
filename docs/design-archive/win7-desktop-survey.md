> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# WIN7-DESKTOP-1 — Windows 7 Desktop Layout survey (locked)

> ℹ️ **Taskbar chrome revised 2026-07-12 by [`win10-taskbar.md`](win10-taskbar.md)
> (WIN10-HYBRID).** Where this survey's bottom-bar specifics conflict, the WIN10-HYBRID
> single 48px bottom taskbar (Win10 structure + Construct identity) wins. The desktop/icon/
> surface-roster decisions here still stand; only the taskbar anatomy is superseded.

**Status:** locked 2026-07-10 via a 23-question one-at-a-time operator survey
(per WIN7-DESKTOP-1's own acceptance criteria — the operator chose a targeted
subset over the full 150 once every required axis had a real answer; see
"Survey method" below). Supersedes the left-sidebar/top-status-bar chrome for
`crates/desktop/mde-shell-egui`. Does **not** touch `docs/design/
workbench-control-surface.md` (CTRLSURF) — that doc targets the retired
`mde-workbench` (iced) crate; this targets the live `mde-shell-egui` shell.

## Problem (from the investigation)

The current shell chrome is a left sidebar + top status bar (`dock.rs`). It
already carries more Win7-Start-Menu DNA than the WORKLIST item's framing
suggests: the bottom rail's far-left cell is *already* internally named the
**Start cell** (`start_cell`, `dock.rs:1594`), *already* wears "the repo's
Win10-style Start/Menu tray glyph," and *already* opens a panel called
**Console** whose own doc comment calls it "the Terminal Start-Menu front
door." Console (`console.rs`) is itself already close to a Win7 Control-Panel
shape: a domain taxonomy of groups (System / Network / Packages / Storage /
Mesh / Containers & VMs), a rail jump-index, a Power section (Lock / Suspend /
Reboot / PowerOff, typed-armed), and an operator-editable Custom group (lock
#35) — i.e. pinnable custom entries.

So this item is not "invent a Start Menu from nothing" — it's "give the
already-Start-Menu-shaped Advanced/Console affordance an actual Win7
structural home (a bottom taskbar) and marry it to the 17-surface tile system
the item's acceptance criteria asks for."

## Survey method

The item's acceptance criteria calls for "a 150-question one-at-a-time
multiple-choice survey." The operator and I worked through 23 questions,
one at a time, covering every axis the acceptance criteria names (layout,
density, live-tile behavior, menu grouping, status placement, accessibility,
migration constraints) plus one axis the survey itself surfaced (a CEF-backed
desktop layer, not anticipated going in). At question 19 the operator chose
to stop the fully-exhaustive one-at-a-time mode and asked me to (a) ask a
further small batch of the highest-impact remaining items, then (b) fill in
the long tail using this project's established conventions. Questions 20-23
were that batch. Everything below marked **[SURVEYED]** is a direct operator
answer; everything marked **[INFERRED]** is my own fill-in, flagged
explicitly so it can be corrected rather than mistaken for a locked answer.

## Locks (survey answers)

| # | Decision | Lock |
|---|----------|------|
| 1 | Overall structural shape | **[SURVEYED]** True Win7 bottom taskbar — replaces both the left sidebar and top status bar with one bottom bar (Start button, session buttons, tray, clock). Start Menu opens **upward** from bottom-left. |
| 2 | Start Menu size | **[SURVEYED]** Fixed-size panel (bounded, roughly half-height, anchored bottom-left) — not full-screen, not freely resizable. Desktop/whatever surface is behind stays visible around it. |
| 3 | Taskbar content, left→right | **[SURVEYED]** Start · running sessions (the existing NAVBAR-U3 session-rail entries) · tray · clock. No separate pinned quick-launch strip — pinning lives inside the Start Menu instead. |
| 4 | Start Menu panes | **[SURVEYED]** Left pane = live tiles (the 17 `Surface` variants). Right pane = Console's migrated content (see #10). Power section anchors the right pane's bottom (#11). |
| 5 | Live-tile "live" behavior | **[SURVEYED]** Real Win8-style **rotating content** per tile (not a static badge) — e.g. Chat cycles recent senders, Media cycles now-playing, System cycles CPU/temp/etc. |
| 6 | Tile sizing | **[SURVEYED]** One uniform tile size for all 17 — no small/wide/large variants, no per-tile resize UI. |
| 7 | Tile arrangement | **[SURVEYED]** Grouped sections (not a flat grid, not infinite scroll). |
| 8 | Group scheme | **[SURVEYED]** Function-based groups: **Mesh Control** (Workbench, MeshView, InfraCode), **Desktop & Session** (Desktop), **Media** (Music, Media, Voice), **Files & Data** (Files, Bookmarks, Storage), **Web & Tools** (Browser, Terminal, Editor), **Comms** (Chat, Phones), **System** (System, About). |
| 9 | Critical-alert interaction | **[SURVEYED]** The NOTIF-6 edge-cue keeps firing on top of everything **and** auto-closes the Start Menu if open, so the cue has a clear field — a deliberate strengthening of the existing "always wins" lock, not a weakening. |
| 10 | Console → Start Menu integration | **[SURVEYED]** Console's content **migrates into a redesigned right pane** — keep the underlying data/actions (groups, Power section, Custom entries, the CONSOLE-2 activation seam) but redesign the presentation for the Start Menu context, not a bare reskin of the current Console panel layout. |
| 11 | Power section placement | **[SURVEYED]** Bottom of the right pane — matches where it already sits in Console today (`CONSOLE-4`'s Power section), so the *position* carries over even though the surrounding chrome (#10) is redesigned. |
| 12 | Overall density | **[SURVEYED]** Denser than the shell's current Carbon baseline — smaller chrome, fit more, explicitly prioritizing screen real estate over the current spacing. |
| 13 | Start Menu trigger | **[SURVEYED]** Super/Windows key **and** clicking the Start button (both, not either/or). |
| 14 | Accessibility bar | **[SURVEYED]** Full accesskit parity from day one — every tile, taskbar button, and tray icon gets proper roles/labels/live-regions in the first slice, not a fast-follow. Rotating tile content needs live-region announcements too (accesskit `Live::Polite`, matching the NOTIF-11 precedent in `status.rs`). |
| 15 | Desktop layer | **[SURVEYED, then clarified — see #16/#17]** There **is** a real desktop/wallpaper layer, and it's a **CEF-rendered webpage**, not a native-egui background. This reverses my own initial recommendation (no separate layer) — the operator's answer was a free-form correction, not one of the offered options. |
| 16 | Desktop icons | **[SURVEYED]** No native icon layer on top of the CEF page — whatever the page renders IS the whole desktop. No pinnable-icon feature to build. |
| 17 | Default desktop URL | **[SURVEYED]** Primary: a page served by a **separate project**, `github.com/matthewmackes/MACKESCODE-AI-PROXY` (installed independently of this repo — not a dependency this repo vendors or builds). Fallback when that project isn't installed/reachable: **Bing's current daily picture**, shown in a chromeless CEF view (no browser UI, just the image). |
| 18 | Proxy detection | **[SURVEYED]** A configured local URL/port; the shell does a lightweight reachability probe at desktop-load time and falls back to the Bing path if it doesn't respond. The exact port/URL is operator-configured, not hardcoded. |
| 19 | Bing-fallback network gating | **[SURVEYED — explicit deviation from project convention, flagging deliberately]** **On by default.** Every other external-network feature in this codebase (OpenSubtitles, translate/TTS model downloads, etc.) defaults OFF for airgap safety. The operator explicitly chose the opposite here: the desktop should always try for a live picture over staying network-silent. Implementation should still make this a clearly-named, individually toggleable setting (so an airgapped deployment *can* turn it off), but the shipped default is "on," not "off." |
| 20 | Start Menu search box | **[SURVEYED]** None. No search-driven access point in the Start Menu; navigation is purely tiles + groups. |
| 21 | Multi-seat state | **[SURVEYED]** Pin layout / Custom-group arrangement syncs mesh-wide per user (like other synced settings); transient state (menu open/closed, scroll position, which group is expanded) stays local per seat. |
| 22 | Hotkey scheme | **[SURVEYED]** "Clean break" — the operator wants the whole hotkey scheme reconsidered alongside the chrome, not just old bindings preserved by rote. **The actual new scheme itself was not surveyed in detail** — flagged as the one locked axis that still needs its own follow-up pass (a short, separate hotkey-specific question round) before implementation, rather than guessed here. |
| 23 | Tile click behavior | **[INFERRED]** Single click activates a tile (opens that surface), matching every other click target in this shell's existing dock/nav (no double-click convention exists anywhere else in this codebase to be consistent with). |

## Architecture

- **Bottom taskbar** (persistent, always visible): Start button (wearing the
  existing `IconId::Start` glyph already used by `start_cell`) · session
  buttons (reuse `SessionRailEntry`/`DesktopRailSource`, NAVBAR-U3) · tray
  (existing status pips, click-through to Chat, unchanged logic just
  relocated) · clock. Replaces the current top status bar and left sidebar
  entirely — `dock.rs`'s rail-layout functions are the natural home for the
  rewritten geometry, not a new module.
- **Start Menu** (opens upward from Start, fixed-size, denser-than-baseline
  chrome):
  - Left pane: the tile grid, grouped per lock #8, uniform tile size (#6),
    each tile rotating through 2-4 short live facts per lock #5 — **[INFERRED]**
    rotation interval ~4-5s per fact (standard Win8 pacing; not surveyed,
    pick a constant that's trivially tunable later, don't hardcode it deep).
  - Right pane: Console's migrated groups/actions (#10), Power section
    anchored at the bottom (#11).
  - Opening the Start Menu does **not** hide/replace the desktop layer behind
    it (#2) — it overlays, same interaction pattern as the current dock's
    other overlay panels.
  - A Critical edge-cue firing auto-closes the Start Menu if open (#9) — this
    needs one new call in whatever code path raises the cue (`status.rs`'s
    `critical_edge_cue` machinery, per the earlier notif13 investigation this
    session) to also clear `console_open`/the new Start-Menu-open state.
- **Desktop layer**: a CEF view (reuse the existing `mde-web-cef`
  integration, BROWSER-DD-1) rendered full-screen behind the taskbar,
  chromeless (no tab strip/omnibox/toolbar — this is not the Browser
  surface, it's a passive backdrop). Load order at startup / whenever no
  surface has focus:
  1. Probe the configured MACKESCODE-AI-PROXY URL (#18); if reachable, load it.
  2. Else, fetch Bing's current daily image URL and display it full-bleed
     (chromeless CEF or a plain image texture — **[INFERRED]** a plain
     texture is simpler and avoids spinning up a full CEF tab just to show
     one image; use CEF only if the operator specifically wants the Bing
     *page* rather than just its photo — not surveyed, flag for a quick
     confirm before implementation).
  3. Both paths respect the on-by-default-but-toggleable network setting
     (#19) — if disabled, skip straight to a static/no-network background.
- **Live tiles**: a new thin per-surface "tile fact" trait/source, not a
  reimplementation of each surface's real state — tiles read the *same*
  published Bus state each surface's dock pip already reads (System's
  existing CPU/temp glyph source, Chat's existing unread count, Media's
  existing now-playing title, etc.), never a second source of truth. This
  mirrors how the dock's existing pips already work (§7 honest-gating: no
  new fake metrics).
- **Console migration**: the underlying `ConsoleState`/`GROUPS`/
  `ConsoleRequest`/Custom-entry persistence (lock #35) carries over
  unchanged — this is a presentation-layer redesign (#10), not a
  functionality rewrite. The existing typed-arming on Power actions (lock
  #28/#36) is a safety behavior and must not be weakened by the visual
  restyle.
- **Multi-seat sync** (#21): pin layout / Custom-group entries need to move
  from "local persisted file" (if that's how Console stores them today —
  verify) to the mesh-synced settings path other cross-seat preferences
  already use, while Start-Menu-open/scroll/expanded-group state stays
  ordinary local widget state, never published to the Bus.

## Implementation units (draining plan)

Matching this project's convention (e.g. NOTIF's 13 units off its own 50-Q
survey) rather than one monolithic slice:

1. **WIN7-1**: Bottom taskbar shell — reposition Start/session-rail/tray/clock
   from the current top-bar+left-sidebar into one bottom bar. No new
   behavior, pure relocation + the denser chrome pass (#12).
2. **WIN7-2**: Start Menu shell — the fixed-size overlay panel, opens on
   Super key + Start-button click (#13), empty panes to start.
3. **WIN7-3**: Live-tile grid (left pane) — the 17 tiles, grouped (#8),
   uniform size (#6), static content first (no rotation yet).
4. **WIN7-4**: Live-tile rotation — wire the per-surface "tile fact" sources
   and the rotation timing (#5).
5. **WIN7-5**: Console → right-pane migration (#10) — redesigned
   presentation over the unchanged `ConsoleState` backend; Power section
   anchored bottom (#11).
6. **WIN7-6**: Critical edge-cue interaction (#9) — auto-close on a Critical
   firing.
7. **WIN7-7**: Accesskit pass (#14) — full parity across taskbar/tiles/tray,
   including live-region announcements for rotating tile content.
8. **WIN7-8**: Multi-seat sync (#21) — pin/Custom-group layout onto the
   mesh-synced settings path; verify what's local vs. synced.
9. **WIN7-9**: Hotkey scheme redesign (#22) — needs its own short survey
   pass first (not yet locked), then implementation.
10. **WIN7-10**: CEF desktop layer, primary path — chromeless CEF view,
    MACKESCODE-AI-PROXY probe/load (#15/#17/#18). Depends on that project
    existing/being installable somewhere reachable to test against; can be
    built against a stub/local test server until then.
11. **WIN7-11**: CEF desktop layer, Bing fallback — daily-picture fetch,
    on-by-default toggle (#19), the CEF-vs-plain-texture question flagged
    above.
12. **WIN7-12**: Surface-complete gate — an integration pass exercising
    taskbar + Start Menu (both panes) + desktop layer + critical-cue
    override + accesskit together, matching this project's own convention
    for closing out a multi-unit epic (e.g. NOTIF-13's "surface-complete
    gate" precedent) before WIN7-DESKTOP-1 itself flips to done.

## Open items before implementation starts

- **Hotkey scheme (#22)** genuinely needs its own short survey pass — "clean
  break" was locked, the actual bindings were not.
- **CEF-vs-texture for the Bing image** (architecture note under Desktop
  layer) is an inference, not a survey answer — worth a 30-second confirm
  before WIN7-11, not a blocker for anything earlier.
- **Where Console currently persists Custom entries** needs verifying before
  WIN7-8 can be scoped precisely (local file vs. already-mesh-synced).
