# NODE-GRADE — per-node capability grade in the vertical dock

Operator-locked 2026-07-04 (20-Q survey). A **stacked mini-list of A–F capability
grades — one per mesh node — displayed ABOVE the notification icons** (the VDOCK-3
status quads) in the left vertical dock. Color-coded green→red; a node at **D or below
blinks**. The grade blends current health with spare headroom so it reads as
"how capable is this node right now."

## Locked decisions (20)

| # | Area | Lock |
|---|------|------|
| 1 | Scope | **Every node**, a stacked mini-list (one grade per mesh node). |
| 2 | Rubric | Four factors: **CPU load/headroom · RAM + disk free · role/worker health + services · mesh reachability.** |
| 3 | Formula | Each factor scored 0–100, **weighted-averaged → A/B/C/D/F bands.** |
| 4 | Colors | **Green→red ramp** (A green · B lime/teal · C yellow · D orange · F red) — `Style` tokens, no raw hex. |
| 5 | Row format | **Colored letter + a tiny load bar** per node. |
| 6 | Blink | **Hard blink on/off** for any node at D or F. |
| 7 | Tap | Tap a node's grade → **that node's Explorer hero card** (EXPLORER-4). |
| 8 | Overflow | Show the **worst N** + a "…" that expands the rest. |
| 9 | Thresholds | **Classic 90/80/70/60** (A≥90, B≥80, C≥70, D≥60, F<60). |
| 10 | Data source | **Reuse the existing mesh telemetry** (unit_aggregator + peer directory — load1/mem/reachability the Explorer already reads); add disk + role-health if missing. Glue (§6), no new probes. |
| 11 | Compute | **Each node self-grades + publishes** its own grade (it knows its disk/services best). |
| 12 | Cadence | **Smoothed ~15–30s** (no flicker on a momentary spike). |
| 13 | Weighting | **Resources heaviest** (CPU/RAM/disk dominant; reachability/role lighter). |
| 14 | Trend | A tiny **↑/→/↓ trend arrow** (score rising/steady/falling over recent samples). |
| 15 | Header | **None** — grades only (self-explanatory letters + colors, saves vertical space). |
| 16 | Reduce-motion | **Always blink** (the alarm outranks the preference). |
| 17 | Meaning | **Blend health + headroom** — an F is *failing OR maxed out*; an A is *healthy AND has room*. |
| 18 | Local node | **Pinned first** with a subtle "you are here" marker. |
| 19 | Sort | Local pinned top, then **worst-grade-first** among peers (F blinking near the top). |
| 20 | Alerting | A node crossing into **D/F also fires a notification into the mesh feed** (CHAT-FIX-2 → reaches the phone), not just the blink. |

## Architecture

### mackesd — the self-grade worker (NODE-GRADE-1)
A new **`node_grade`** worker (rank-0, universal — every node grades itself), reusing the
telemetry the platform already gathers:
- **Factor scores (0–100 each):** CPU (load1 vs cores, headroom), RAM (free%), disk
  (free% on `/`), role/worker health (expected `worker_role` workers running + no failed
  services), mesh reachability (overlay up + lighthouse/peer reach). Each blends
  **health + headroom** (#17): a maxed resource scores low even when "healthy".
- **Weighted average** (resources heaviest, #13) → a 0–100 score → an A–F band
  (90/80/70/60, #9). A **smoothed** value over ~15–30s (#12) + a trend (last-N slope, #14).
- **Publish** `{grade, score, factors{}, trend}` to the substrate at
  `<root>/node-grade/<hostname>.json` (the SEC-5 mesh-shunt / KDC-pairing replication
  pattern — every peer reads every node's grade). 
- **On a transition into D or F**, emit an `event/notify/<source>` alert (CHAT-FIX-2
  producer) so the drop reaches the Chat feed + the phone (#20). Debounced against flapping.
- Census: `("node_grade", 0)` in `WORKER_TIERS` (rank-0).

### mde-shell-egui — the dock grade list (NODE-GRADE-2)
In `dock.rs`, render a **grade mini-list in the dock's bottom zone, ABOVE the VDOCK-3
status quads** (a new band between the app groups and the status quads):
- Read the published per-node grades (the same bus/aggregator the dock's status inputs use
  — extend `set_status_inputs`/`StatusInputs` with the grade set, §7 honest pre-poll dim).
- Stack rows: **local node pinned first** with a marker (#18), then peers **worst-first**
  (#19). Each row = the **A–F letter** in the green→red `Style` token (#4) + a **tiny load
  bar** (the score) (#5) + the **trend arrow** (#14). ~grade cell sized to the quad idiom.
- **D/F blinks** hard on/off via `Motion` (#6/#16 always-blink). 
- **Tap → Explorer hero** for that node (route to `Surface::MeshView`/the Explorer focused
  on the node id, reusing the EXPLORER jump path) (#7).
- **Overflow**: worst-N visible + a "…" more-popup (#8) — reuse the dock's overflow idiom.
- No header (#15). All colors/metrics via `Style` tokens (§4).

### Shared tokens (NODE-GRADE-3, if missing)
The A–F **green→red ramp** as named `Style` tokens (`GRADE_A..GRADE_F`) in `mde-egui`,
with a backing test; the dock + any future grade UI consume them. Coordinate with the
existing status/accent palette (no new hues if the ramp already exists).

## Acceptance (runtime-observable; per task)
- The dock shows a stacked A–F grade per mesh node above the status quads; the local node
  is pinned first; peers sort worst-first; each row has the colored letter + load bar +
  trend arrow.
- Grades come from each node self-grading (published to the substrate) on a smoothed
  ~15–30s cadence, from the four weighted factors (resources heaviest), classic bands.
- A node at D/F **blinks**; tapping a node opens its Explorer hero; overflow shows worst-N + "…".
- A node crossing into D/F posts a notification to the Chat/notify feed.
- All via `Style`/`Motion` tokens (§4); the worker is census rank-0; honest dim pre-poll.

## Risks
- **Serialize with the VDOCK churn** — NODE-GRADE-2 edits `dock.rs`'s bottom zone, which
  VDOCK-3/4/6b also touch. Land it on the settled vertical-dock base (after VDOCK-6b
  re-applies), or coordinate the bottom-zone layout.
- **Self-grade trust** — a node publishes its own grade; a compromised/wedged node could
  lie or stop publishing. A stale/absent grade must read honestly (a greyed "?" / stale
  marker), and "unreachable" is itself an F from the observer's side (blend the published
  grade with the observer's reachability).
- **Blink fatigue** — a persistently-D node blinking forever is noise; consider an
  acknowledge/quiet after the notify fires (still colored, stops blinking once seen). (Flag
  for the operator; default = keep blinking per #16.)

## Out of scope (v1)
- Configurable thresholds/weights (ship the locked defaults; settings later).
- Historical grade charts (the trend arrow is the only history surfaced).

## Tasks → `docs/WORKLIST.md` NODE-GRADE-1..3 (+ a live-smoke verify).
