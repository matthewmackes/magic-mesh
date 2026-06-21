# Worklist schema — the self-replicating queue

The skill's memory buffer AND queue. Durable file:
**`docs/design/GUI-REFINE-WORKLIST.md`** (alongside `docs/WORKLIST.md`). Items
enter ONLY through `scripts/append-item.sh` via plan-validate-execute. This is the
single chokepoint that keeps self-replication bounded.

## Contents
1. Item format
2. Status markers
3. changes.json (the intermediate)
4. Admission rules (validate · dedup · threshold · budget)
5. The round log

---

## 1. Item format
Each item is a Markdown list line under a surface heading, carrying its full
provenance so a later round (or the operator) can act on it without re-deriving:

```
- [ ] **<id> · <surface> · <criterion>** — <proposed-change>.
      before: <before-state>; accept: <machine-checkable acceptance>; value: <0-100>.
```

- **id** — `RCG-<surface>-<NNN>` (e.g. `RCG-workbench-007`), monotonic per surface.
- **surface** — the GUI view (workbench/files/music/voice-hud/applet) + slug.
- **criterion** — the exact rubric clause violated (e.g. `carbon:focus-ring`,
  `carbon:spacing`, `iced:idle-park`, `carbon:contrast`).
- **before-state** — what's wrong now (the finding), with the measured value.
- **proposed-change** — the one scoped edit.
- **acceptance-criteria** — MACHINE-CHECKABLE (a `score-surface.sh` delta, a gate,
  a contrast ratio, idle-CPU≈0). No "looks nicer".
- **value** — projected leverage 0–100 (criticality × reach × confidence). Used by
  the value threshold + ranking.
- **status** — see below.

## 2. Status markers
Mirror the main worklist: `[ ]` open · `[>]` in progress · `[✓]` done ·
`[blocked] <reason>`. A reverted change → `[blocked]` with the one-line reason
(do not silently drop it; the operator may want it).

## 3. changes.json (the intermediate)
Before any mutation, candidate items are written to `changes.json` and validated
**before** they land — a machine-verifiable, debuggable intermediate state. Shape:

```json
{ "candidates": [
  { "surface": "mde-workbench:fleet.hardware",
    "criterion": "carbon:focus-ring",
    "before": "primary button has no 2px $focus border (focus invisible)",
    "change": "wrap the button in focus_ring() from mde-theme",
    "accept": "score-surface.sh focus_ring_missing count drops by 1; gates green",
    "value": 78 } ] }
```

## 4. Admission rules (the chokepoint — `scripts/append-item.sh`)
A candidate is admitted only if ALL hold:
- **VALIDATE** — every required field present + well-typed against this schema;
  `criterion` is a known rubric clause; `value` ∈ [0,100].
- **DEDUP** — not a duplicate/overlap of an existing item (same surface + criterion
  + similar change). Reject near-duplicates so the queue can't spawn forever.
- **THRESHOLD** — `value ≥ MIN_VALUE` (default 30). Low-leverage findings are
  dropped (or left for the operator), not queued.
- **BUDGET** (global, hard) — refuse once any cap is hit:
  - `MAX_ITEMS` open at once (default 40),
  - `MAX_NEW_PER_RUN` admitted this invocation (default 8),
  - and the loop's own `MAX_ROUNDS` / farm-job budget.
  Hitting a cap is a STOP + operator note, never silent truncation.

## 5. The round log
Per surface, append one line per round so plateaus are visible:
```
round N | item RCG-... | score N-1 → N | gates green/red | ACCEPT/REVERT
```
The score column is the convergence anchor: 2 consecutive rounds with no gain →
PLATEAU stop. All-gates-green + score ≥ target → CONVERGENCE stop.
