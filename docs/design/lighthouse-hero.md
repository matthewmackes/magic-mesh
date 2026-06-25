# LIGHTHOUSE — Hero focus on lighthouses (design)

**Status:** locked via 25-Q operator survey, 2026-06-18.
**Trigger:** Operator — "Hero Focus on Lighthouses throughout the system. ALWAYS use Carbon. New Feature: add, at the bottom of the Notifications Hub, an area focused on lighthouse health. Each lighthouse a square icon, animated like a lighthouse (circling beam), Carbon Green = healthy, Red = unhealthy. Clicking takes the operator to a dedicated 'Lighthouses' tab under Mesh in the Workbench."

The lighthouses are the relay/anchor nodes of the Nebula overlay (today: the
two DigitalOcean nodes — `45.55.33.179`, `159.65.183.51`). This
epic gives them a first-class, animated presence in the desktop and a dedicated
operations tab.

> **Note (post-SUBSTRATE-6):** this design predates the substrate split and treats
> the lighthouse "master/shadow" as the **`lizardfs-master`** SPOF + a promotable
> shadow. **LizardFS is removed.** A lighthouse's "core service" health is now
> **nebula up + its etcd member healthy** (coordination quorum), and files sync via
> Syncthing with no master. Read the `lizardfs-master` health-input + the
> master/shadow badge / "promote shadow → master" action below as their etcd
> equivalents (etcd quorum / leadership-lease holder); there is no LizardFS master
> to promote.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| 1 | Identify lighthouses | **Directory descriptor role** — the `role` field in each peer's QNM-Shared directory JSON (`role == lighthouse`). Fleet-consistent, already replicated. |
| 2 | Health data source | **`mesh-status.json` snapshot** — reuse the existing replicated presence + service-health snapshot (≈30 s); no new probes. |
| 3 | Unhealthy (red) condition | **Offline OR a core lighthouse service down** (nebula / etcd member health — was `lizardfs-master` pre-SUBSTRATE-6). Catches the coordination SPOF, not just presence. |
| 4 | Hero scope (where else) | **Broad** — Hub section + Workbench tab + bash welcome Network Overview + a panel applet/indicator + reuse the PLANES hero line-art + **wallpaper**. (Hub + tab are this epic's core; the others are follow-on hero surfaces, see §Hero-scope.) |
| 5 | Hub placement | **Pinned footer** — always visible at the Hub bottom, below Music/SIP, like the Music playback bar. |
| 6 | Square layout | **Square + detail per item** (beacon left, name/IP/status). |
| 7 | Which / overflow | **All `role==lighthouse` nodes, horizontal scroll** — the pinned footer is a horizontally-scrollable strip of square+detail cards (reconciles 5+6+7: fixed-height footer, scrolls sideways as lighthouse count grows). |
| 8 | Section header | **"Lighthouses" label + a Carbon beacon hero glyph** + live healthy/total count. |
| 9 | Square icon | **Abstract rotating beacon** (a light source, not a building). |
| 10 | Animation | **Rotating conic beam sweep** — a wedge of light sweeping 360° around the square. |
| 11 | Animation cadence | **Healthy = slow sweep; unhealthy = fast strobe.** |
| 12 | Frame model | **Discrete stepped rotation** (≈12 positions) — cheap, still reads as circling; advanced by an iced `time` subscription, paused when the footer isn't visible. |
| 13 | Healthy color | **Dedicated Carbon Green 50 token** — add `green_50` (or `beacon_healthy`) to `mde-theme` with a backing palette test (§4). |
| 14 | Unhealthy color | **`mde-theme` `danger` token** (Carbon Red). |
| 15 | Health states | **Strictly binary green/red** (per operator). No-data folds to red. |
| 16 | Row detail | **Name + overlay IP + status word.** |
| 17 | Open mechanism | **Bus deep-link, spawn-if-needed** — publish a deep-link (e.g. `event/workbench/open` with `{panel:"lighthouses", focus:<id>}`); a running Workbench switches tabs, else `mde-workbench` is spawned at that panel. No duplicate windows. |
| 18 | Tab position | **Right after Peers** in the Mesh group. |
| 19 | Click target | **Whole row** opens the tab. |
| 20 | Per-lighthouse focus | **Open the tab, focus/scroll to the clicked lighthouse** (others still listed). |
| 21 | Tab content | **Full card** — overlay + public IP, handshake state, peers-connected count, uptime, CA/cert expiry, role, core service status. |
| 22 | Master/shadow | **Yes — leader/follower badge + failover-readiness** (which anchor holds the etcd leadership lease + is quorum healthy — was the `lizardfs-master` SPOF pre-SUBSTRATE-6). |
| 23 | Tab actions | **Full ops** — restart services, open SSH/remote, promote-shadow-to-master; **each confirmed** before acting. |
| 24 | Tab refresh | **Bus subscription (push)** — update in real time as health records land. |
| 25 | Tab hero | **Hero line-art band + a row of animated beacons** across the top, summarizing fleet lighthouse health. |

## Architecture

### Shared lighthouse model (`mde-theme` + a small shared helper)
- **Carbon token:** add `green_50` to the `mde-theme` palette (Carbon Green 50)
  with a palette test asserting its RGBA (§4 — no raw hex outside `mde-theme`).
  The beacon's healthy color reads this token; unhealthy reads `danger`.
- **Lighthouse discovery + health** is derived from the data the Hub and
  shell already consume:
  - lighthouse set = directory descriptors where `role == lighthouse`;
  - per-lighthouse health = the `mesh-status.json`/snapshot presence + service
    flags → green iff online AND nebula up AND (for an anchor) its etcd member is
    quorum-healthy (was `lizardfs-master` up pre-SUBSTRATE-6); else red.
  - A pure `fn lighthouse_health(node) -> Beacon{healthy: bool, ...}` so it is
    unit-testable and shared by the Hub footer, the applet, and the tab.

### Notification Hub footer (`mde-workbench` `mde-notify-center` bin)
- A new **pinned footer** section (below Music/SIP), a horizontally-scrollable
  strip of lighthouse cards. Header: beacon glyph + "Lighthouses" + `N/M`.
- Each card: the **animated conic-beam beacon square** + name + overlay IP +
  status word.
- Animation: an iced `time` subscription advances a discrete `beam_step`
  (mod 12); healthy steps slowly, unhealthy strobes; the subscription is
  **inactive when the footer/Hub isn't shown** (no idle CPU).
- Whole-card press → publish the `event/workbench/open` deep-link with
  `focus=<lighthouse id>`.

### Workbench "Lighthouses" tab (`mde-workbench`)
- New `Panel::new("lighthouses", "Lighthouses")` registered in the **Mesh**
  group **immediately after `peers`** (`model.rs`).
- Hero band (PLANES Nebula/Lighthouse line-art via `panel_chrome::hero_band`)
  + a row of the same animated beacons.
- Per lighthouse: a **full card** (overlay+public IP, handshake, peers, uptime,
  cert expiry, master/shadow badge, failover readiness, core services).
- **Bus subscription** for push refresh.
- **Full ops** actions, each behind a confirm: restart services (over the
  overlay), open SSH/remote, promote shadow → master.
- Honors the deep-link `focus` (scroll/highlight the clicked lighthouse).

### Deep-link plumbing
- The Hub and Workbench share an `event/workbench/open` lane (`{panel, focus}`).
  Workbench subscribes; on receipt it selects the panel (and focus). If no
  Workbench is running, the Hub falls back to spawning
  `mde-workbench --panel lighthouses [--focus <id>]`.

### Hero-scope follow-ons (lock 4 — broad)
Core of this epic = Hub footer + Workbench tab. The remaining hero surfaces are
carried as their own tasks so the core can ship first:
- **Bash welcome Network Overview** — highlight lighthouses in the ASCII
  diagram (beacon marker + master/shadow + health) (`mesh-welcome.py`).
- **Panel applet/indicator** — a small lighthouse-health indicator on the
  Cosmic panel (worst-of green/red across lighthouses).
- **Wallpaper** — a lighthouse hero motif in the mesh wallpaper.
- **Service hero art reuse** — the PLANES Nebula/Lighthouse line-art as the tab
  banner (already in lock 25).

## Acceptance (high level; per-task bullets in the worklist)
- The Hub shows a pinned, always-visible Lighthouses footer with one animated
  beacon per `role==lighthouse` node, green when healthy / red when a lighthouse
  is offline or a core service is down, horizontally scrollable.
- The healthy beam sweeps slowly; an unhealthy beam strobes.
- Clicking a lighthouse opens the Workbench Lighthouses tab (spawning it if
  needed) focused on that lighthouse.
- The Lighthouses tab lists every lighthouse as a full card with master/shadow +
  failover status, refreshes via bus push, and offers confirmed restart / SSH /
  promote-shadow ops.
- All colors come from `mde-theme` tokens (the new `green_50` + `danger`), with
  a palette test (§4). No raw hex outside `mde-theme`.

## Risks / notes
- **Carbon Green 50 token** is a §4-governed change — must land with a palette
  test, not a raw literal in the applet/panel.
- **Animation cost** on the low-RAM shadow lighthouse (~948 MB): the discrete
  stepped model + "pause when hidden" keeps it cheap; verify on that node.
- **promote-shadow-to-master** was destructive (`lizardfs-master` failover) — with
  LizardFS removed (SUBSTRATE-6), the analogue is moving the etcd leadership lease /
  reseating an anchor; still confirm-gate it and treat as the riskiest action.
- **"Strictly binary" + no-data→red** can flash red on a stale snapshot at
  startup; mitigate by treating "no snapshot yet" as red only after the first
  successful read, otherwise neutral until data arrives.

## Out of scope
- Auto-failover (the tab offers a *manual, confirmed* promote; automatic leader
  failover is the existing QNM leader-election concern, not this epic).
- Adding/removing lighthouses (provisioning) — this epic visualizes + operates
  existing lighthouses.
