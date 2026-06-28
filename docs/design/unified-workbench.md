# Unified "Workbench" — Design Requirements

**Status:** design brief (for handoff to Claude Design) · **Date:** 2026-06-28 · **Epic:** UNIFY
**Supersedes:** FRONTDOOR `Q40` (apps stay standalone) and `Q68` (summoned-only); evolves `NAV-1` (sidebar IA).
**Authority:** locked via a 3-round operator survey (2026-06-28). Newest wins (per `AI_GOVERNANCE.md`).

> Hand this document to Claude Design. It defines **what to design** (screens, components,
> states, layout, visual system, behavior) and the **hard constraints** the design must
> respect. The Rust/process architecture is summarized only where it shapes the UI.

---

## 1. Product context (one paragraph)

MCNF is a secure, no-fixed-center **workgroup mesh** platform: a small set of trusted
machines (≤ 12 nodes — 3 lighthouses + 9 peers) joined into one encrypted Nebula overlay,
coordinated by etcd, replicated by Syncthing, running on a **Cosmic / Wayland** desktop. The
**Workbench** is the operator's cockpit for the whole mesh. Today the GUI is fragmented
across ~8 separate binaries; this epic consolidates them into **one always-on Workbench**.

## 2. The mandate

Design **one always-on application** — a persistent main window **plus a few owned overlay
surfaces** — that **replaces every other GUI** in MCNF (the lone exception is the Music
player, which stays standalone). Navigation is **VS Code-style: a thin activity bar + a
dense tree/list navigator + a content pane**. The visual language is **strict, dense IBM
Carbon**, tuned for a **DevSecOps / full-stack power user**.

## 3. Target user & design principles

**Persona:** a DevSecOps / full-stack engineer operating a live mesh fleet — comfortable with
IDEs, terminals, and admin consoles; wants information density, structure, and speed over
hand-holding.

**Principles:**
1. **Information-dense.** Structured lists, tree views, zebra-striped tables, hairline
   dividers, compact rows. Reward the power user; never waste vertical space.
2. **Keyboard-first.** Everything reachable without the mouse: command palette/omnibox,
   activity-bar shortcuts, tree keyboard navigation, deep-linkable views.
3. **Structure-visible.** Show the hierarchy (trees, breadcrumbs, grouped lists) rather than
   hiding it behind wizards.
4. **Glanceable truth.** Health/status is always visible and **honest** — loading, empty,
   degraded, and error states are first-class; never fake an "all good."
5. **Quiet, not flashy.** Motion is subtle and presentation-only; status comes from Carbon
   semantic tokens + small pips/badges, not decorative color.

## 4. Visual system (HARD constraints)

- **IBM Carbon Design System**, strictly. Dark theme = Carbon **Gray 100 / Gray 90**; light
  theme = Carbon **Gray 10**. Follow the OS theme by default.
- **Only Carbon tokens. No raw hex anywhere.** Color, spacing, radii, shadows, motion all
  come from the design-token set (engineering enforces this with a CI lint that rejects raw
  color literals). Deliver specs as **token references**, not hex values.
- **Typography:** IBM Plex Sans (UI) + IBM Plex Mono (IDs, IPs, hashes, paths, logs). Carbon
  type scale.
- **Density:** ship a **Compact** default and a **Comfortable** option. Design both. Compact
  is the power-user default: tight row heights, dense tables, zebra striping.
- **Status & semantics:** use Carbon semantic tokens (success / warning / error / info) for
  state; small status **pips/LED dots** and **tags/badges** for inline status; monospace for
  machine values.
- **Motion:** Carbon motion tokens only; subtle, fast (respect a reduce-motion cap of
  ~80 ms); **never blocks input or work**. Skeleton loaders for async, crossfades for
  swaps. No bouncy/decorative animation.
- **No skeuomorphism / retro chrome.** (An earlier "late-2000s flair" idea was explicitly
  **set aside** in favor of strict modern dense Carbon.)
- **Iconography:** Carbon icons; consistent stroke weight; legible at activity-bar size.

## 5. Shell layout & information architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│ HEADER:  ⌘K omnibox / command palette   ·   mesh-health summary   ·        │
│          now-playing glance   ·   node identity / theme                     │
├──┬───────────────────────┬─────────────────────────────────────────────────┤
│A │  TREE / LIST           │                                                 │
│C │  NAVIGATOR             │            CONTENT PANE                         │
│T │  (collapsible,         │      (the selected item's full surface)         │
│I │   zebra rows,          │                                                 │
│V │   multi-level)         │                                                 │
│B │                        │                                                 │
│A │                        │                                                 │
│R │                        │                                                 │
└──┴───────────────────────┴─────────────────────────────────────────────────┘
   Owned overlay surfaces (NOT inside the window — design as separate surfaces):
   • Toasts (top layer, transient)   • Voice/Calls HUD (bottom-right, anchored)
   • Mesh-map wallpaper (desktop background)   • Cosmic panel applet (system panel)
```

- **Activity bar** (far left, thin, icon-only): one icon per **domain**. Selecting a domain
  loads its tree into the navigator. Keyboard-shortcutted.
- **Tree/list navigator** (collapsible): the dense, zebra-striped, multi-level tree/list of
  the selected domain's contents. This is the workhorse — design its states richly.
- **Content pane:** the selected item's full surface.
- **Header:** global omnibox/command palette, a compact mesh-health summary, a **now-playing
  glance** (for the standalone Music app), and node identity / theme controls.

### Activity-bar domains to design

| # | Domain | Tree/navigator contents | Content pane |
|---|--------|------------------------|--------------|
| 1 | **Overview / Home** | quick links, pinned views | The always-on **home**: live tiles (mesh health, build farm, datacenter, peers, alerts) on a snap grid + the command omnibox. *(This is the former summoned "Front Door," now the persistent home.)* |
| 2 | **This Node** | local subsystems | local node status, services, config, boot-readiness |
| 3 | **Mesh** | peers directory (tree), certs, enroll | peer detail, topology, enrollment, CA/cert ops |
| 4 | **Fleet** | nodes, jobs, waves | multi-node operations, job runner, results |
| 5 | **Provisioning / Datacenter** | dom0s, VMs, tofu roots | VM lifecycle, tofu plan/apply (gated), DO control |
| 6 | **Files** *(absorbs mde-files)* | file/artifact **tree**, mounts, peer shares | browser w/ thumbnails, detail, transfer |
| 7 | **Voice / Calls** *(absorbs mde-voice-hud)* | call history, contacts | active-call control + dialer; **plus** the bottom-right HUD overlay |
| 8 | **Notifications** *(absorbs notify-center/toasts)* | alert log, filters, DND | Action Center; **plus** the top-layer toast overlay |
| 9 | **Apps** *(the launcher)* | local + peer apps, groups, favorites | launch/raise apps; cross-node launch |
| 10 | **System** | settings, audit, logs | preferences, audit trail, log viewer |

## 6. Screens & components to deliver (the actual design output)

Design each in **both** Gray 10 (light) and Gray 90/100 (dark), in **Compact** density (and
note Comfortable deltas):

1. **Shell chrome** — activity bar + tree navigator + content pane + header. The skeleton
   everything lives in. Show selected/hover/focus states; collapsed navigator; resize.
2. **Overview/Home** — the live-tile dashboard: tile sizes (S/M/L/XL on a snap grid), a tile
   anatomy (title, live value, sparkline/mini-viz, status pip, action affordance), and the
   command omnibox.
3. **Tree navigator patterns** — collapsible groups, multi-level indent guides, zebra rows,
   selection/hover/focus, inline status pips, counts/badges, filter field, density.
4. **Data-dense table** (use the **Peers directory** as the exemplar) — sortable columns,
   zebra striping, monospace IDs/IPs, status pips, row actions, sticky header, selection.
5. **Files surface** — file tree + detail + thumbnail grid + breadcrumb + mounts/shares.
6. **Voice/Calls** — the in-Workbench Calls surface (history + active call) **and** the
   bottom-right **HUD overlay** (compact, anchored, auto-dismiss).
7. **Notifications** — the **Action Center** (grouped alert log, DND toggle, filters) **and**
   the **toast** overlay (transient, top-layer, stacking).
8. **Apps launcher** — app grid/list with fuzzy search + filter chips (Favorites / Apps /
   Mesh / Workloads / Services), real icons, operator-curated groups, cross-node target.
9. **Cosmic panel applet** — a compact mesh/lighthouse-health indicator + quick actions that
   lives in the OS panel and summons/raises the Workbench.
10. **Mesh-map wallpaper** — a live, EtherApe-style topology rendered as the desktop
    background (legible behind windows; subtle).
11. **Component states for EVERY surface** — **loading/skeleton**, **empty**, **error (with
    retry)**, and **partial/degraded**. This is required, not optional (honesty principle).
12. **Command palette / omnibox** — unified search across apps + files + mesh + actions, with
    ranked results and keyboard navigation.

## 7. Interaction & behavior requirements

- **Always-on lifecycle:** the window **never closes**; the Super key **raises/focuses** the
  existing window (it does **not** toggle it away or relaunch). Design the raise/focus
  transition, not a cold-launch splash. It autostarts on any node with a display.
- **Keyboard model:** command palette (e.g. `⌘/Ctrl-K` and/or Super), activity-bar number
  shortcuts, full tree keyboard nav, focus-visible everywhere.
- **Deep-linking:** every item is addressable so a notification, the applet, or a CLI can
  jump straight to a surface (preserve existing focus-slug semantics).
- **Cross-node context:** surfaces can target a **remote node** (a node selector affordance);
  show clearly when you're viewing another node's data.
- **Density toggle:** Compact ↔ Comfortable, applied globally.
- **Confirm-gated actions:** mutating mesh/datacenter actions go through a typed
  **confirmation gate** (propose → review → confirm). Design that confirm pattern.

## 8. Engineering constraints that shape the UI (designer awareness)

- **Toolkit:** libcosmic / iced on the Cosmic desktop. Map components to Carbon-styled
  libcosmic widgets; a GPU canvas is available for the tile grid and mesh map.
- **Overlays are real Wayland layer-shell surfaces** — toasts (top layer), the Voice HUD
  (anchored), and the wallpaper (background) are **separate surfaces**, not modals inside the
  window. Design them as standalone surfaces with their own anchoring/lifecycle.
- **One process owns all surfaces**, with **per-surface fault isolation** — so each surface
  must have a self-contained error state (one surface failing must not blank the others).
- **Carbon-token-only styling is CI-enforced** — provide token references in redlines, never
  hex. Motion durations must reference motion tokens.
- **Cosmic owns the shell** (panel, lock screen, greeter, global settings) — the Workbench
  integrates **into** Cosmic; it does **not** replace desktop chrome.

## 9. Out of scope (do NOT design)

- **Full Music library/player UI** — Music stays a **standalone app**; only design the
  header **now-playing glance** (read-only: track, art, transport) that deep-links to it.
- **Rich AI/Copilot UI** — the backend publisher isn't live yet; design only a launcher-level
  entry, not a fabricated AI surface (no mock data).
- **Speech-to-text UI** — deferred (no airgapped engine).
- **Cosmic shell replacement** — panel/lock/greeter/global-settings are Cosmic's.
- **Accessibility beyond visual** — a separate track; but **do** follow Carbon contrast,
  focus-visible, and full keyboard operability now.

## 10. Locked decisions (operator survey, 2026-06-28)

| # | Decision | Lock |
|---|----------|------|
| 1 | Consolidation approach | **One binary, all surfaces** — `mde-workbench` becomes THE single GUI; persistent main window + embedded panes + owned overlays. Replaces all other GUI binaries (except Music). |
| 2 | "Always-on" meaning | **Autostart + persistent + self-heal** — launches at session start, never closes (Super raises, not toggles), relaunches on crash. |
| 3 | Process/architecture | **One `cosmic::Application`, many surfaces** (main window + child windows + owned layer-shell overlays). |
| 4 | Surfaces absorbed | **Files, Voice/Calls, Notifications, the Apps launcher, all mesh-ops panels** (Cosmic applet + wallpaper kept but owned by the one binary). |
| 5 | Music | **Standalone exception** — keeps its own app/window; Workbench shows a now-playing glance only. |
| 6 | Applet + wallpaper | **Kept, owned by the one binary** — applet stays a panel resident (summon/status); wallpaper stays a Background layer-shell surface. |
| 7 | Role-gating | **Any node with a display** (not role-limited); headless nodes are a no-op. |
| 8 | Navigation/IA | **Activity bar + tree** (VS Code style): icon rail → dense zebra tree → content pane. Evolves NAV-1's flat sidebar. |
| 9 | Visual language | **Strict, dense, modern IBM Carbon** (Gray 10/90/100, tokens only). DevSecOps power-user density: structured lists, tree views, zebra striping. "Late-2000s flair" set aside. |
| 10 | Delivery | **Big-bang single release** — old binaries (`mde-files`, `mde-voice-hud`, `mde-notify-*`) removed and folded in together in one cutover. |

## 11. Acceptance (what "designed" means before build)

- Shell chrome + all 10 domains have approved mockups in light + dark, Compact density.
- Every surface has loading / empty / error / degraded states designed.
- A documented **component inventory** (activity bar, tree, table, tile, toast, HUD, Action
  Center, applet, command palette, confirm gate) with **token references** (no hex).
- Overlay surfaces (toast, HUD, wallpaper, applet) designed as standalone surfaces.
- Keyboard map + focus model documented.

## 12. References

- IBM Carbon Design System (color, type, motion, components, a11y).
- Existing in-repo: `docs/design/front-door.md` (the live tile system being made
  persistent), `docs/design/app-launcher-rethink.md`, `docs/design/workbench-nav-grouping.md`
  (NAV-1), `docs/design/motion-system.md`, and the `mde-theme` Carbon token set.

---

*Next step after design: lift these surfaces into `docs/WORKLIST.md` as the `UNIFY` epic
(user-story tasks + runtime-observable acceptance, per `AI_GOVERNANCE.md` §7) and record the
FRONTDOOR Q40/Q68 supersede in `docs/DECISIONS.md`. Deferred until the visual design returns
and the operator gives the go-ahead.*
