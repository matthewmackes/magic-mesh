# MENUBAR-ALL — a unified title + menu bar on every surface

Operator-locked 2026-07-04 (15-Q survey). Give **every primary nav-bar surface** a
consistent top bar: a **large stylized UPPERCASE workspace title** on the left, the
**menus inline to its right**, and a **live per-surface status cluster** on the far
right — one shared `mde-egui` component so all 15 surfaces read as one platform.

## Governing principle (operator, 2026-07-04) — the reason the menu bar exists

> **The menu bar's job is to surface to the operator ALL controls — including the
> advanced and complex ones — not just the common actions.** It is the operator's
> complete, discoverable control surface for each workspace. This is **especially
> critical for the OpenStack component interfaces** (the IaC workspace), where the
> menus carry the full standard-API verb set. Every control a surface can perform
> should be reachable from its menus; nothing important lives only behind an
> undiscoverable chord. (Still bound by §7 — an item ships only when its real seam
> exists; comprehensiveness never means dead/stub entries.)

## Locked decisions (15)

| # | Area | Lock |
|---|------|------|
| 1 | Scope | **Every primary nav-bar surface** — all 15 in the vertical dock (Workbench, Mesh-View, Instances, Desktop, Music, Media, Files, Voice, Browser, Terminal, Editor, Chat, System, Storage, About). |
| 2 | Title style | **Large mono display title**, accent-tinted — the IBM Plex Mono DISPLAY tier (the EDTB-7 heading ramp), tinted with the surface's category accent. |
| 3 | Layout | **Title left · menus inline right · live status cluster far-right** — one left-anchored strip (title + menus) with a per-surface status cluster on the right. |
| 4 | Implementation | **One shared `mde-egui` MenuBar component** — a `MenuBar` + title-header + status-cluster widget every surface embeds (passing its title/accent/menus/status). Terminal + Editor refactor onto it. |
| 5 | Menu spine | **Shared File/Edit/View/Help spine + surface-specific menus** — a consistent spine where it applies, plus each surface's own menus (Voice→Call, Music→Playback, Browser→History…). |
| 6 | Status cluster | **Live per-surface state** — each surface's real indicators (Voice: peers/codec; Music: now-playing/output; Browser: engine/security; Terminal: session/host; Mesh-View: node count), wired to real state (§7). |
| 7 | Existing bars | **Refactor Terminal (TERM-MENUBAR-1) + Editor (Word-97) onto the shared component** — same items + seams, unified rendering. |
| 8 | Honesty | **Honest omit/disable (§7 no dead entries)** — an item ships only when its seam exists; a context-needing item (Copy w/ no selection) renders disabled; a genuinely-absent feature is omitted. |
| 9 | Keyboard | **Alt-mnemonics + live shortcut hints** — Alt+F opens File, underlined mnemonics, each item shows its live keymap-resolved shortcut (like Terminal's bar). |
| 10 | Title action | **Decorative identity only** — the title is not clickable (the dock handles surface switching). |
| 11 | Placement | **Slim top bar inside each surface panel** — fixed consistent height at the top of each surface's panel area (right of the vertical dock). |
| 12 | Motion | **Shared `mde_egui::motion`** — hover highlight, dropdown open-spring, item press feedback; reduce-motion aware. |
| 13 | Non-app surfaces | **Their real seams, honestly** — viewers/settings get menus for what they genuinely do (Mesh-View→View/Node; Desktop→Session/Display/Input; Instances→Instance/Power; System/Storage→the settings categories; Workbench→Plane/View); spine where it fits, no invented menus. |
| 14 | Title text | **UPPERCASE** — VOICE, BROWSER, FILES, MESH VIEW, WORKBENCH. |
| 15 | Phasing | **One big wave** — build the shared component + all 15 surfaces' bars together (each surface a file-disjoint unit within the wave). |

## Architecture

### The shared component — `mde_egui::menubar` (MENUBAR-ALL-1)
A new module in `crates/shared/mde-egui`:
- **`MenuBar`** — the slim top bar: renders the UPPERCASE mono display title (accent-tinted,
  the DISPLAY tier), the inline menu strip, and the right-aligned status cluster, at one
  consistent height. Consumes only `Style`/`Motion`/`fonts` (§4).
- **The menu model** — a caller-supplied tree: `Menu { title, mnemonic, items: [MenuItem] }`,
  `MenuItem { label, shortcut_hint, enabled, on_activate }` (an item is present only when its
  seam exists — the caller omits absent ones and passes `enabled=false` for context-gated
  ones, §7). Dropdowns open with the shared motion spring; **Alt-mnemonics** + full keyboard
  nav; each item's **live shortcut** renders on the right (resolved by the caller from its
  keymap so a rebind reflects).
- **The status cluster** — a caller-supplied `Vec<StatusChip>` (icon/text/tone) rendered
  right-aligned; the surface fills it from its live state each frame.
- **Reduce-motion aware**, DPI-crisp, focus-ring on every menu item (a11y, lock 5 of Construct).

### Per-surface embedding (MENUBAR-ALL-2..16, one big wave)
Each surface crate embeds `MenuBar` at the top of its panel, passing:
- its **UPPERCASE title** + **category accent** (the dock group's accent token),
- its **menu tree** — the shared **File/Edit/View/Help** spine (only the items it truly has)
  **+ its surface-specific menus**, every item bound to a **real existing seam** (§6 glue,
  no new behavior), surfacing **all** its controls incl. advanced ones (the governing
  principle), honestly omitted/disabled per §7,
- its **live status cluster** (real per-surface indicators).

**Terminal** (`mde-term-egui/menubar.rs`, now carrying TERM-MENUBAR-1 + the TMUX-FC-2 Tmux
menu) and **Editor** (the Word-97 bar) **refactor onto the shared component** — same menus +
seams, unified look. The **IaC workspace** (see `iac-workspace.md`) is the extreme case: its
menu bar is **dynamic per-service menus from the OpenStack catalog** carrying the full
standard-API verb set — the governing principle's headline use.

## Acceptance (runtime-observable; per task — §7)
- Every one of the 15 surfaces renders the slim top bar: an **UPPERCASE mono accent title**
  left, menus inline, a **live status cluster** right, at one consistent height, via the
  **shared `mde_egui::MenuBar`** (Terminal + Editor included, refactored onto it).
- Each surface's menus = the **File/Edit/View/Help spine (where real) + its surface menus**,
  every visible item **driving a real seam** (comprehensive incl. advanced ops); a
  context-gated item is **disabled**, an absent one **omitted** — no dead entries.
- **Alt-mnemonics** open menus, each item shows its **live shortcut**; hover/open/press use
  **shared motion**; the status cluster reflects **real live state**.
- Non-app surfaces (viewers/settings/Workbench) carry menus for **their real seams**.
- All colours/metrics/motion from `mde_egui` (style-leak grep clean, §4); the title is
  decorative; the bar renders correctly at 1.0 + a fractional scale.

## Risks
- **The shared component must fit 15 very different surfaces** — the menu model + status
  cluster must be general enough (Terminal's tmux tree vs Browser's history vs IaC's dynamic
  catalog menus) without becoming a leaky abstraction. Design the model against the two
  hardest existing cases (Terminal, Editor) first even though the rollout is one wave.
- **"Surface ALL controls" vs §7 no-stubs** — comprehensiveness must never mean shipping a
  greyed "coming soon"; every menu item maps to a landed seam or is omitted. The IaC catalog
  menus make this load-bearing.
- **Refactoring the working Terminal/Editor bars** risks regressing landed menus (TERM-
  MENUBAR-1 + TMUX-FC-2 + the editor set) — keep every existing item + seam, change only the
  host widget; gate on their existing menu tests.
- **Vertical dock coexistence** — the platform's dock is vertical-left with no top chrome
  today; the new per-surface top bar must not fight the dock gutter/overlap rules (VDOCK).
- **Title height vs content** — a large display title in a *slim* bar needs a tuned type
  size so it's bold but doesn't eat content height.

## Out of scope (v1)
- A global application menu bar (macOS-style, one bar for the whole shell) — this is
  **per-surface**, matching the workspace model.
- Right-to-left / full-i18n menu localization (English-first).
- User-customizable menu contents (the menus are the surface's real seams, not user-editable).

## Tasks → `docs/WORKLIST.md` MENUBAR-ALL-1 (shared component + Terminal/Editor refactor) then MENUBAR-ALL-2..N (one per surface, the one-big-wave rollout).
