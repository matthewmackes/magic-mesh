# PEERS-DT — Peers page as a Carbon data table

> **HISTORICAL / SUPERSEDED IN PART (2026-07-19):** describes the retired `mde-workbench` Peers panel (`peers.rs`). The live desktop is the egui-native, DRM-native shell `mde-shell-egui` — see [`quasar-vdi-desktop.md`](quasar-vdi-desktop.md). Read the `mde-workbench` crate references below as historical.

**Epic prefix:** `PEERS-DT`
**Status:** locked 2026-06-15 (operator 5-question survey)
**Reference:** Carbon Design System — Data table usage
**Surface:** `crates/workbench/mde-workbench/src/panels/peers.rs`

## Why

Today the Peers page is a status-grouped list (This machine / Online / Idle /
Devices) with a side detail panel. The operator wants the canonical **Carbon
data-table** layout: a flat, sortable table with a toolbar, per-row status tags,
and expandable rows for actions — denser, sortable, and consistent with the
Carbon look (§4).

## Locked decisions (survey)

| # | Fork | Decision |
|---|------|----------|
| 1 | Status handling | **Flat + sortable Status column** — one table; status is a sortable column rendered as a colored Carbon tag (Online/Idle/Offline). No group sections. |
| 2 | Columns | **Name · Status · Role · Overlay IP · Latency · Services · Last seen** |
| 3 | Selection | **Single-select → detail** — no multi-select checkboxes / batch-action toolbar. |
| 4 | Row detail | **Carbon expandable row** — click the chevron to expand inline, revealing the peer's actions (Ring / Send file / Open / lifecycle) + details (battery, presence, paired state) below the row. Replaces the side detail panel. |
| 5 | Toolbar | **Search + Refresh only** — the existing filter becomes the Carbon search input; a Refresh button. No add-peer (stays on Registration) / density / column-config. |

## Resulting layout

```
┌ Peers ───────────────────────────────────────────────────────────────┐
│ [ Search peers…                                            ] [Refresh] │
├───────┬──────────┬───────┬─────────────┬─────────┬──────────┬─────────┤
│ Name ▲│ Status   │ Role  │ Overlay IP  │ Latency │ Services │ Last    │  ← sortable headers
├───────┼──────────┼───────┼─────────────┼─────────┼──────────┼─────────┤
│▾ fedora │ ●Online │ Lighth│ 10.42.0.3   │ 2 ms    │ 4        │ now     │
│    [Ring] [Send file] [Open] · battery — · presence online           │  ← expanded row
│▸ UNIT-EAGLE │ ●Online │ Workst │ 10.42.0.4 │ —     │ 5        │ now     │
│▸ Lighthouse-02 │ ●Online │ Lighth │ 10.42.0.1 │ 14 ms │ 3      │ 5s      │
│▸ Lighthouse-01 │ ○Idle   │ Lighth │ 10.42.0.2 │ 18 ms │ 3      │ 1m      │
└───────────────────────────────────────────────────────────────────────┘
```

- **Severity/status tags** + all color via `mde-theme` Carbon tokens (no raw hex, §4).
- **Sortable** by any column (Name/Status/Role/Latency/Last seen); default sort
  Status (online first) then Name.
- **Expandable row** carries everything the old side panel had (per-peer actions
  + metrics + KDC controls for device rows).
- Data source unchanged: the PD-1 directory (`mackesd peers --json` / the bus
  directory) + the PD-6 latency cache + PD-8 metrics.

## Acceptance (runtime-observable, §7)

1. Peers renders as one flat Carbon data table (no group sections); rows come
   from the live directory (no demo data).
2. Column headers sort the rows (asc/desc); default sort is Status then Name.
3. Search filters rows live (name / role / IP / service match).
4. A row expands inline (Carbon chevron) to show the peer's actions + details;
   collapses again. Exactly one detail mechanism (no leftover side panel).
5. All status tags + accents are `mde-theme` tokens; `cargo test -p mde-workbench`
   green; the §4 hex lint stays clean.
6. Device rows (paired phones) expand to the KDC actions (Ring / Send file / etc.).

## Out of scope

- Multi-select / batch actions (rejected — Q3).
- Add-peer / density toggle / column-config in the toolbar (rejected — Q5).
- Pagination (≤12-peer envelope; not needed).
- The live-metrics backend itself (Netdata not deployed — separate bug, task #45).
