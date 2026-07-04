# PICKER-GROUPS — grouping the bottom picker's surface icons

Operator-locked 2026-07-04 (10-Q `/plan`, iteratively refined). The Win10 taskbar app
row (NAVBAR-W10-2, `dock.rs`) is a flat icon-only run of all 15 surfaces. This epic
organizes it into **named, vertically-labeled groups** — each group's heading is a
rotated label to the LEFT of its icons, acting as both label and divider.

## The taxonomy (locked)

**Workbench stands alone as the lead anchor** (first icon, before the grouped run),
then **six labeled groups** in this left-to-right order:

| Order | Group | Surfaces (in Surface::ALL relative order) |
|---|---|---|
| lead | *(Workbench)* | Workbench — standalone, no label |
| 1 | **Comms** | Chat, Voice |
| 2 | **Workloads** | Instances |
| 3 | **Terminals** | Editor, Terminal, Browser |
| 4 | **Mesh** | MeshView, Desktop |
| 5 | **System** | Files, System, Storage, About |
| 6 | **Media** | Music, Media |

All 15 surfaces are placed (About kept, in System). Notes on the non-obvious calls
(operator, this survey): Instances→Workloads but Desktop→Mesh (a VM is a workload, a
remote desktop is a mesh connection); Editor/Terminal/Browser form the new **Terminals**
group (interactive session windows); Files→System (with Storage); Workbench leads alone.

## Styling locks

| # | Decision | Lock |
|---|----------|------|
| L1 | Label orientation | **Bottom-to-top** — rotated 90° CCW, reading upward (chart-axis convention). |
| L2 | Label placement | To the **left** of each group's icons (the operator's core ask). |
| L3 | Divider | **Label + a hairline rule** — the vertical label sits beside a thin rule; the **hairline is Carbon Blue** (the interactive-blue token, `Style` — no raw hex, §4), with generous padding before/after each group. |
| L4 | Label color | **Per-group accent** — each label takes its group's accent color; these SAME accents key the Explorer's category identity (EXPLORER O8) — one color language across picker + explorer. Carbon tokens. |
| L5 | Section order | **Comms · Workloads · Terminals · Mesh · System · Media** (after the Workbench lead). |
| L6 | Compact | **Overflow chevron** — when the bar can't fit every group + label + the right-side tray, groups that don't fit collapse into a `»` overflow popup at the end (least-used/last groups go first); the tray stays pinned right. |
| L7 | Within-group order | Preserve each surface's existing `Surface::ALL` relative order inside its group. |

## Architecture

`crates/desktop/mde-shell-egui/src/dock.rs` — the `taskbar()` app-row render:
- A `Group { label: &'static str, accent: Color32-token, surfaces: &[Surface] }` table
  (const) defining the six groups + order (L5/L7); Workbench rendered first as the
  standalone lead (no group).
- Render loop: for each group — a rotated (bottom-to-top, L1) label text laid out to the
  left, a Carbon-blue hairline rule (L3), then the group's icon cells (the existing
  24px cell render, unchanged), then padding. Label painted in the group accent (L4).
- New accent tokens per group live in `mde-egui::Style`/`mde-theme` (§4, shared with
  EXPLORER-15's category identity — define once, both consume).
- **Overflow (L6):** measure the grouped run against available width (total − Workbench
  lead − tray reserve); groups that overflow fold into a trailing `»` chevron button
  whose popup lists the hidden groups (their labels + icons). Reuse the tray's existing
  anchored-popup idiom.
- The existing active-underline, hover-fill, click-routing, and `Surface::ALL` semantics
  are unchanged — this is a layout/grouping pass over the same cells.

## Acceptance (runtime-observable)
- The app row renders Workbench first, then the six groups in the locked order, each
  preceded by its rotated bottom-to-top accent label + a Carbon-blue hairline.
- Every one of the 15 surfaces appears exactly once, in its locked group (About in
  System); clicking any icon still routes to its surface (unchanged).
- Labels are colored by group accent; the same accent tokens are the ones EXPLORER-15
  uses for category identity.
- At a narrow width the overflowing groups collapse into a `»` chevron popup; the tray
  stays pinned right; nothing is silently dropped.
- All via Carbon tokens (§4 — no raw hex, incl. the blue hairline); shell tests updated.

## Risks
- **Vertical text in egui** — rotated `LayoutJob`/galley rotation; verify legibility +
  hit-box correctness at 40px bar height (labels are display-only, not clickable).
- **Width budgeting** — the overflow measurement must account for the label widths +
  hairlines + the pinned tray; get the reserve right so the chevron appears before
  clipping, not after.
- **Accent token sharing** — coordinate the six accent tokens with EXPLORER-15 so both
  epics define/consume one set, not two.

## Out of scope
- Reordering surfaces by drag; user-customizable groups (the taxonomy is fixed here).
- Changing the tray (right side) or the active/hover/route behavior.

## Tasks → `docs/WORKLIST.md` PICKER-1..3.
