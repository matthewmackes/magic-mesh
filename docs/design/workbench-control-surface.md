# MCNF Workbench — Control Surface redesign (CTRLSURF)

> **HISTORICAL / SUPERSEDED IN PART (2026-07-19):** the Control-Surface *concepts* (Command Watchfloor, compact↔expand, unified search/ladder/catalog/pin store) carry forward, but this doc targets the retired `mde-workbench` crate. The live desktop is the egui-native, DRM-native shell `mde-shell-egui` — see [`quasar-vdi-desktop.md`](quasar-vdi-desktop.md). The `mde-workbench` crate paths and `cargo test -p mde-workbench` DoD below are stale.

**Status:** locked 2026-06-29 via the operator design tour + two synthesis
workflows (the 10-paradigm control-surface design `wf_13ac8b02`, and the nav
taxonomy rethink `wf_f45a892b`) + a convergence survey. Supersedes the ad-hoc
Front Door shell and the partially-shipped NAV-1
(`docs/design/workbench-nav-grouping.md`).

## Problem (from the investigation)

The Front Door (`crates/workbench/mde-workbench/src/panels/front_door.rs`,
~10.4k lines) and the 58-panel nav grew four structural drifts:

1. **Triplicated input + duplicated relevance.** Three separate search inputs
   (`right_pane` 5513, `fullscreen_view` 5323, `launcher_view` 4404) and **two**
   relevance ladders (`search::score_match` 1206 + `launcher::fuzzy_score` 1629)
   over **two** disjoint catalogs (4 hardcoded launcher tiles 2905-2908 + the
   live `action/apps/*` aggregate).
2. **Compact/expand is unmodeled.** `Mode` (Panel/FullScreen) is an *in-window
   content swap* (`mode_toggle` 5739), not a real window resize — so the
   operator's "open compact like Win10 Start, expand arrow to full screen"
   decision has nowhere to live.
3. **Keyboard nav is launcher-only.** `launcher_key_subscription` (3258) is
   registered only while the launcher overlay is open; the home/tile grid has no
   arrow-key story.
4. **Two stacked left navs + icon-less tiles.** The Front Door renders its own
   `rail()` (5390: Pinned/Surfaces) *inside* the content, beside the main
   `sidebar.rs`; the canvas `TileGrid` (7943) draws flat rects with **no app
   icons** and a loading-skeleton that blanks even the static launcher tiles.

Plus the nav itself: 3 SHOUTING labels (`OTHER NODES`, `MESH: PROVISIONING`,
`MESH: VIRTUAL WORKLOADS`), a two-way "provisioning" collision, a 14-panel Mesh
group, and flat (no sub-group) rendering.

## Locks (survey answers)

| # | Decision | Lock |
|---|----------|------|
| 1 | Headline paradigm | **Command Watchfloor** — a unified intent line fused to a status-first ambient board (synthesis of Command Cockpit + WATCHFLOOR) |
| 2 | Compact surface (default) | The **new** command line + ~5 ambient status rows, keyboard-native, latency-masked |
| 3 | Expand surface (full screen) | The **familiar Win10 two-pane rail + grid**, refined (keep + fix the tile grid; do NOT retire the canvas) |
| 4 | Compact↔Expand | A real `CompactExpand` **window-size** enum wired to an actual resize via the expand arrow — replaces the in-window `mode_toggle` |
| 5 | First slice (M2) | Unified ladder module + compact popup **+** the Expand "what changed" activity rail |
| 6 | Nav taxonomy | **Folded in** — the scope-first hybrid relabel + sub-groups (below) |
| 7 | Unification | One search line, one verb-aware ladder, one catalog, one pin store (favorites/groups) across BOTH modes |
| 8 | Keyboard | Promote `launcher_key_subscription` (3258) to the whole home (Up/Down highlight, Enter commit, Esc back-scope-then-close, Tab/Ctrl+1..5) |
| 9 | Honest metrics | Keep the `FrontDoorData`/`TileKey`/`mod project` backbone (no fake values, §7); status rows + tiles read the existing bus paths |
| 10 | Density | **Subtle** — remove the worst dead space (redundant triple titles, empty output bands); overall Carbon breathing room kept |
| 11 | Lists | **Zebra striping** everywhere — lift the `mde-notify-center` idiom into a shared `striped_list` helper; route list/table panels through it |
| 12 | Icon | New mesh-native Workbench icon (concept A "Mesh Control" glyph + matching launcher tile) replacing the generic Carbon tools glyph |
| 13 | Rollout | Phased + feature-flagged, additive per phase, validation gate between phases (5-second status test; live-verify, not just green tests) |

## Architecture

- **Compact mode** (default, Win10-Start-sized window): the Command Watchfloor —
  one always-focused command line over ~5 ambient status rows projected from
  `FrontDoorData::read` (842) via `mod project` (969-1092). Cache-first +
  bounded `APPS_*_TIMEOUT` + the `responded` mesh-down flag + generation-guarded
  Copilot streaming **below** the instant local hits — latency-masked by
  construction, no Bus read on the hot path.
- **Expand mode** (full screen via the expand arrow → real resize): the familiar
  two-pane **rail + tile grid**, refined — the *single universal sidebar* (the
  Front Door rail's Pinned/Surfaces fold into `sidebar.rs`), the tile grid kept
  but **fixed** (real icons, the loading-skeleton no longer blanks static
  launcher tiles), plus a "what changed" activity rail driven by the peers
  directory-changed push + the 15s `poll_subscription` (3246).
- **Shared spine** (both modes): the unified verb-aware relevance ladder (merging
  `score_match` + `fuzzy_score`, seeded from the existing score tests 9102-9169),
  one catalog, one pin store (`action/apps/favorites`), the `CompactExpand`
  window-size enum augmenting `Mode` (373), and whole-home keyboard nav.

## Nav taxonomy (folded in — scope-first hybrid)

Blast-radius ordering with two functional cross-cuts kept whole (Monitoring,
System). Sub-group headers added to `model.rs` (the model has none today).

```
Overview      Home  (+ operator-pinnable "most used" strip)
This Node     ▸ Hardware & Desktop   ▸ Network
Mesh          ▸ Fabric  ▸ Shared Resources  ▸ Services  ▸ Local Network  ▸ Join the Mesh
Fleet         ▸ Roster  ▸ Orchestration  ▸ Node Templates
Datacenter    New Virtual Machine · Datacenter        (highest blast radius, last)
Monitoring    (unified)
System        ▸ Configuration  ▸ Maintenance  ▸ Preferences & Help
```

Plain-language renames (selection): `OTHER NODES→Fleet`, `MESH: PROVISIONING→
Mesh ▸ Join the Mesh`, `MESH: VIRTUAL WORKLOADS→dissolved (Fleet ▸ Node
Templates + Datacenter)`, `Mesh Services→Mesh Connection`, `New Mesh→Create a
Mesh`, `Mesh Pending→Join Requests`, `Mesh Federation→Linked Meshes`, `Mesh
Storage→Shared Storage`, `All Services→Service Directory`, `SIP Gateway→Voice
Gateway`, `VM Spawner→New Virtual Machine`, `Config→Apply Configuration`,
`Health→Health Check`, `Resources→Resource Usage`. Rule (from the verb lens):
**kill "provisioning" as a label** — enroll / create / run-a-VM are different
verbs.

## Phased migration (no workflow breaks)

- **Phase 0** (no behavior change): land the unified ladder + parser behind a
  feature flag; existing `view()` (4337), the 4 hardcoded tiles, both ladders
  stay live.
- **Phase 1**: add the command line + ambient status rows as the Compact right
  pane, reusing `FrontDoorData`; keep the existing grid reachable.
- **Phase 2**: promote keyboard nav across the home (additive, no removals).
- **Phase 3**: make favorites/groups the single pin store, surfaced as frecency
  top candidates; keep the static rail in parallel, A/B against regulars' deck.
- **Phase 4**: the `CompactExpand` window-size enum + wire the expand arrow to a
  real resize; retire the in-window `mode_toggle`.
- **Phase 5**: fold the Front Door rail into the one universal sidebar; apply the
  nav taxonomy + sub-group headers; ship zebra striping + the subtle density
  pass + the new icon. (Tile grid stays — Expand is the familiar rail+grid.)

## Acceptance (runtime-observable)

- One search input and one relevance ladder serve both modes (the 3 inputs / 2
  ladders are gone); unit-tested ladder reproduces the seeded score tests.
- The expand arrow performs a real window resize between a compact and a
  full-screen size (not an in-window swap); state survives reopen.
- Up/Down/Enter/Esc/Tab/Ctrl+1..5 drive the home with the launcher closed.
- Compact never reads the Bus synchronously on keypress (cache-first; ~120ms
  debounced async preview); mesh-down shows the `responded=false` state, not a
  hang.
- The left nav renders one universal sidebar (no second in-content rail) with the
  scope-first sections + sub-group headers, plain-language labels, no SHOUTING.
- Tiles show real icons; the loading skeleton never blanks a static launcher
  tile; lists render zebra-striped via the shared helper.
- `cargo test -p mde-workbench` green; renders through `mde-theme` Carbon tokens
  (§4); farm-built.

## Out of scope (do NOT build yet)

The full 2D watchfloor canvas-cursor board; inline destructive verbs (kill/
uninstall) in the fast keyboard flow until a confirm/undo gate exists; any
AI/Copilot in the hot path beyond the existing below-the-fold streaming card;
Cosmic-panel-hosted dropdown chrome until the window-size state machine is
proven; the orthogonal paradigms (Spatial Mesh Map, Radial/HUD,
Conversational-first, Notebook/Runbook, Tiling wall).

## First tickets (lift to `docs/WORKLIST.md` as `### CTRLSURF`)

- **CTRLSURF-1** — unified verb-aware relevance ladder as a standalone unit-tested
  module (merge `score_match` + `fuzzy_score`, seed from tests 9102-9169).
- **CTRLSURF-2** — the Compact popup: one command line + ~5 ambient status rows
  from `FrontDoorData`/`mod project`, cache-first, async-preview, never a sync
  Bus read.
- **CTRLSURF-3** — promote `launcher_key_subscription` to the whole home.
- **CTRLSURF-4** — the Expand "what changed" activity rail (peers-directory push +
  15s poll).
- **CTRLSURF-5** — `CompactExpand` window-size enum + expand-arrow real resize.
- **CTRLSURF-6** — one universal sidebar (fold the Front Door rail in) + the nav
  taxonomy + sub-group headers + plain-language relabel.
- **CTRLSURF-7** — shared `striped_list` (zebra) helper + route list/table panels.
- **CTRLSURF-8** — subtle density pass (drop triple titles, flex fixed-height
  output boxes) + the new Workbench icon.

## Source

10-paradigm investigation/matrix/roadmap: workflow `wf_13ac8b02` (transcript in
the session subagents dir). Nav taxonomy: `wf_f45a892b`. Both are captured here;
the full agent outputs remain in the task output files.
