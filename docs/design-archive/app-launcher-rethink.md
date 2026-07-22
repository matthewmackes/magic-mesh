> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# Application Launcher — rethink (APPLAUNCH)

**Status:** Locked (50-question survey, 2026-06-27). **Headline:** the Applications
Applet is not rethought in isolation — it is **consolidated into the Front Door**.
One launcher: the Front Door is the fast **local Start-menu** *and* the mesh-wide
resource launcher; the standalone `mde-apps-applet` is folded in and retired. §0
"Simple": kill the two-surface split.

## Vision (Q50)
**One coherent launcher** — no duplication between the panel apps-applet and the
Front Door. The Start button / Super key opens the Front Door in **panel mode** (a
compact quick-launch dropdown) that expands to its existing full-screen mode.

## Locks (the 50)

| # | Area | Lock |
|---|------|------|
| 1 | Core role | **Local Start-menu first** (mesh resources secondary) |
| 2 | IA | **Unified searchable list + filter chips** (Local / Mesh / Workloads / Services) |
| 3 | Landing | **Favorites grid** |
| 4 | Categories | **Operator-curated groups** |
| 5 | Search match | **Fuzzy / typo-tolerant** (on top of the exact>prefix>word ladder) |
| 6 | Search scope | **Name + keywords + description** (.desktop Keywords/Comment) |
| 7 | Peer apps | **Auto-discovered** (a node's real installed `.desktop` set) |
| 8 | Local launch | **Focus-or-launch** (raise running window, else spawn) |
| 9 | Cross-node | **Remote-desktop session** (remmina) to the peer |
| 10 | On-peer | App on peers-but-not-local → **badge + launch-on-peer** |
| 11 | Workloads | **Keep as a filter chip** (start/stop/attach inline) |
| 12 | Services | **Service card** (status + metadata) |
| 13 | Recents | **None** (no usage history) |
| 14 | Favorites | **Per-user, mesh-synced** (QNM-Shared) |
| 15 | Layout | **33% portrait dropdown** (panel mode) |
| 16 | Render | **Favorites grid + list rows** |
| 17 | Power menu | **Win+X, slimmed to essentials** (Terminal/Settings/Files/Power + →Workbench) |
| 18 | Run box | **Merged into search** (`>` prefix) |
| 19 | Per-app menu | **Properties / Copy command / Open location / Uninstall** |
| 20 | Default scope | **Local-first browse**, mesh via the filter chip |
| 21 | Icons | **Real app icons** (icon-theme lookup); Carbon glyphs for mesh/workload/service |
| 22 | Keyboard | **Full keyboard-first** (↑↓/Enter/Esc/Tab chips/number-jump) |
| 23 | Motion | **Richer** Carbon motion (staggered entrance, icon pop) within 3Hz/reduce-motion |
| 24 | Trigger | Super-key **toggle** |
| 25 | Offline | Mesh-down → **hide mesh entries** (local fully works) |
| 26 | Start button | **Brand logo + subtle badges** |
| 27 | Performance | **Cache + background refresh** |
| 28 | A11y | Default (keyboard-first covers most; no dedicated bar) |
| 29 | vs Front Door | **MERGE — one launcher** (retire the standalone applet) |
| 30 | Theme | **Follow Cosmic theme** (Gray 10/90/100) |
| 31 | Settings | **In Cosmic Settings** (not a bespoke applet settings) |
| 32 | All-apps | **= the Apps filter** (no separate mode) |
| 33 | Panel button | Opens **Front Door panel mode** → expand to full-screen |
| 34 | RD client | **remmina** |
| 35 | Result interaction | **Click-to-expand row** (host chips + actions) |
| 36 | First-run | **Helpful empty states** (reuse FD greeting sentinel) |
| 37 | Pinning | **Pin + optional pin-to-panel** |
| 38 | Peer publish | **On-demand query** when focusing a node (not constant publish) |
| 39 | Ranking | **Pure relevance** |
| 40 | System toggles | **None** (Cosmic panel owns volume/wifi/brightness) |
| 41 | Multi-monitor | **Primary monitor** |
| 42 | Migration | **Fold role into the Front Door, then retire `mde-apps-applet`** at parity |
| 43 | Install | **None / out of scope** |
| 44 | Run command | **`>` prefix** runs via shell |
| 45 | Voice | **Reuse the Front-Door voice slice** (FRONTDOOR-15) |
| 46 | Notifications | **Notification Hub owns it** (Start-badge hint only) |
| 47 | Privacy | **No usage tracking** |
| 48 | Service-card actions | Open + status + copy endpoint **+ restart-if-owned** |
| 49 | Perf budget | **Open <150ms, cache-first + lazy-mesh** (a slow peer never blocks the open) |
| 50 | Success metric | **One coherent launcher** |

## Architecture

The Front Door (`crates/workbench/mde-workbench/src/panels/front_door.rs`, already
the full-screen launcher/omnibox/tiles surface) gains the **app-launcher view** the
applet had — a unified, filter-chipped, fuzzy-searched list landing on a favorites
grid — rendered in its **Panel mode** (33% dropdown) and **FullScreen mode**.

- **Data** stays the `apps_aggregator` (mackesd `ipc/apps.rs`, `action/apps/list`):
  local `.desktop` scan (real-icon resolved), `running-apps.json` badging, workloads
  (`compute-inventory.json` union), services (PD-2 descriptors → cards). **Cache +
  background refresh** (Q27/49): open paints from the last list instantly; aggregation
  refreshes async; **mesh sections lazy-load** so a slow peer never blocks the open.
- **Peer apps (Q7/38):** auto-discovered by **on-demand query** — when the operator
  focuses a node (Mesh filter / node detail), query that peer's `.desktop` set live
  (a new `action/apps/peer-list` RPC the peer answers), surfaced with an **on-peer
  badge** and launch-on-peer (remmina RD via the existing `action/apps/launch`).
- **Search (Q5/6/39/44):** fuzzy + name/keywords/description, pure-relevance ranked;
  a leading `>` runs the rest as a shell command (the merged Run box).
- **Launch (Q8/9):** focus-or-launch local (`wmctrl` raise); remmina RD for peer apps.
- **Groups (Q4):** operator-curated buckets (a per-user QNM `app-groups.json`),
  collapsible sections in the Apps filter.
- **Migration (Q29/42):** build the launcher view into the Front Door, repoint the
  Start button + `event/apps/toggle` (Super key) to open the Front Door panel mode,
  reach parity, then **delete `mde-apps-applet`** (mirrors FRONTDOOR-16 retiring the
  old launcher). Keep `mde-cosmic-applet`'s *lighthouse-health* applet (separate).

## Acceptance (runtime-observable)
- Super key / Start opens the Front Door **panel mode** on the primary monitor in
  <150ms, landing on the favorites grid; expand control switches to full-screen.
- Typing fuzzy-matches local apps by name/keywords/description with real icons;
  `>cmd` runs a shell command; Esc closes; full keyboard nav works mouse-free.
- The Mesh filter lists peers; focusing one queries its apps on-demand and shows
  them with an on-peer badge; launching opens a remmina RD session.
- Workloads filter starts/stops/attaches; a service entry opens a card with status +
  copy + (if owned) restart.
- Mesh down → mesh entries hidden, local launcher fully functional.
- `mde-apps-applet` is removed; nothing references it; the Front Door is the sole launcher.

## Risks
- **Parity-before-retire:** must hit applet feature-parity in the Front Door before
  deleting the binary (FRONTDOOR-16 pattern) — else a regression in the only launcher.
- **On-demand peer query latency:** a slow/dead peer must time out without blocking
  the UI (lazy-mesh + the offline-hides-mesh lock cover this).
- **Real-icon lookup cost:** icon-theme resolution must be cached (perf budget).

## Out of scope
In-launcher package install (Q43), system quick-toggles (Q40), recents/usage
tracking + telemetry (Q13/47), a built-in RD viewer (remmina shells out), notification
surfacing (the Hub owns it).
