---
name: refining-carbon-gui
description: >-
  Iteratively redesign and improve the FEEL, polish, and UX of the MCNF
  iced/Cosmic GUIs against IBM Carbon and Rust/iced best practice — a
  self-replicating worklist that critiques a rendered surface, makes ONE scoped
  improvement, re-renders to verify, and appends its own follow-up items. Every
  round is grounded in the XCP build+render farm (xcp-build.sh gates +
  preview-capture render) and machine-checkable Carbon/iced rubrics, never vibes.
  TRIGGER ONLY when the operator explicitly types "refine the GUI", "improve the
  look/feel", "polish the UI", "iterate on the Carbon design", or
  "/refining-carbon-gui <surface>". NOT for a single scoped UI edit (just do it),
  draining the functional backlog (/ship), a one-off render check (/preview), or
  a release cut (/release).
---

# refining-carbon-gui — self-replicating Carbon/iced GUI refinement loop

Operator-gated (like `/release`): run only when the operator asks to refine the
GUI. It iterates on **one surface** at a time — critique a rendered view, make a
single scoped improvement, re-render + gate to verify it actually advanced, then
bound-replicate follow-up items. The objective function is the machine-checkable
Carbon + Rust/iced rubrics in `reference/`, scored by `scripts/score-surface.sh`
— **the farm and the rubric decide accept/reject, never the model judging its own
prose.**

Anchored to the rulebook: **§4** (Carbon look, tokens single-sourced in
`crates/shared/mde-theme`, no raw hex / off-scale metrics — lint-gated), **§6**
(new code is glue, reuse crates; no mesh→deleted-shell dep), **§7** (a GUI change
is done when it builds + tests green + renders through `mde-theme` tokens; the
on-session visual gate was lifted 2026-06-11, so a render is verification, not a
blocker). Heavy compute farms to XCP (operator directive 2026-06-20) — the local
AI host stays idle.

> **Surface** = one GUI view. The whole skill speaks in surfaces. A surface maps
> to a crate + a render slug:
> `mde-workbench` (panels, slugs like `fleet.hardware`, `maintain.audit`, `''`=home) ·
> `mde-files` · `mde-music` · `mde-voice-hud` · `mde-cosmic-applet` (launcher).

## Prerequisite — the build+render farm

This skill cannot run without a registered XCP build+render slot (it builds +
headless-renders there). Confirm one exists:

```sh
./install-helpers/xcp-build.sh slots          # a reachable slot must be listed
```

If none, stand one up first (see `docs/ops/xcp-build-farm.md`) — that is operator
setup, not part of this loop. With no slot, STOP and say so; do not fake renders.

## The loop (one surface, one change per round)

**0 · SELECT & BASELINE.** Resolve the surface ($1 / the operator's named one) to
its crate + slug. On the farm: `xcp-build.sh cargo build -p <crate> --slot <s>`,
then `xcp-build.sh render '<slug>' --slot <s>` → pull the PNG. **Read the PNG.**
Run `scripts/score-surface.sh <png> <crate>` for the baseline objective score +
findings JSON. Log round 0 — the convergence anchor.

**1 · CRITIQUE** (evaluator role — grounded in EXTERNAL signal, not self-prose).
Combine (a) the deterministic findings from `score-surface.sh` +
`lint-carbon-tokens.sh` (off-scale px, raw hex, non-Carbon easing, layout
animation, missing states, sub-threshold contrast) with (b) a Read of the PNG
against `reference/carbon-criteria.md` + `reference/rust-iced-criteria.md`.
Produce a RANKED list of specific actionable findings (what · which criterion ·
why), highest-leverage first. Persist it to the worklist (below) so later rounds
build on it.

**2 · PICK ONE** (admission control). Take the single highest-value finding that
clears the value threshold and isn't a duplicate/overlap of an existing worklist
item (`scripts/append-item.sh` dedups). **One change per round — never batch** —
so the score delta is attributable to that change.

**3 · IMPROVE** (generator role). Make the one scoped edit. Carbon edits go
through `mde-theme` tokens ONLY (§4 — no raw hex / off-scale px outside
`crates/shared/mde-theme`). Motion/iced edits follow `reference/rust-iced-criteria.md`
(time-driven + idle-parked, opacity/transform/color only, reduce-motion branch,
one animation backend). New code is glue; reuse crates (§6).

**4 · GATE + RE-RENDER** (the farm verifies, not the model). On the farm:
`xcp-build.sh gates --slot <s>` (fmt + clippy + test) and `cargo test -p mde-theme`
for any token change; the run also covers `lint-carbon-tokens.sh` +
`lint-mesh-boundary.sh` (carbon, boundary gates). Then re-render
`xcp-build.sh render '<slug>' --slot <s>` → new PNG, re-run `score-surface.sh`,
**Read the new PNG.** For motion changes, spot-check the running binary's idle CPU
returns to ≈0 (no continuous redraw).

**5 · ACCEPT or REVERT** (the measurable-value gate). If score(N) > score(N-1)
**AND** gates green **AND** the render visibly advances the criterion → ACCEPT:
commit named pathspecs, why-not-what message + the `Co-Authored-By: Claude Opus
4.8 (1M context) <noreply@anthropic.com>` trailer; flip the item `[✓]`. Else →
REVERT (`git checkout` the pathspecs), mark the item `[blocked]` with a one-line
reason. Same fix retried at most **3×**, then SOFT-ESCAPE to the operator. **Never
`git push` / `/release` / cutover — those stay operator-gated.**

**6 · SELF-REPLICATE (bounded).** From the residual findings + anything the
accepted change surfaced, write candidate follow-ups to `changes.json` and run
`scripts/append-item.sh` — it validates against the schema, dedups against the
existing worklist, enforces the per-item value threshold + the GLOBAL budget (max
new items / rounds / farm jobs), and only then admits them. **This is the only
place new items enter the worklist.**

**7 · CONVERGENCE CHECK → LOOP or STOP.** Update the round log. If a stop
condition is hit, STOP with a factual summary + a before/after render gallery +
the remaining items. Otherwise return to step 1 on the next-highest item.

## Hard stop / convergence conditions (all three + the escapes)

- **MAX ROUNDS** per surface (default 8) — the only cap from Anthropic's "Building
  effective agents"; the next two are this skill's additions on top of it.
- **PLATEAU**: stop after **2** consecutive rounds with no objective-score gain —
  treat no-improvement as a stop, not a reason to churn.
- **CONVERGENCE**: all hard gates green (build/clippy/test, carbon, boundary,
  contrast, focus-ring) **AND** score ≥ the surface target.
- **SOFT-ESCAPE** to the operator: a fix fails 3×; a blocker; a low-confidence /
  subjective finding (a "beauty" call with no machine signal); the global budget
  is exhausted.

## Self-replicating worklist

Durable file: **`docs/design/GUI-REFINE-WORKLIST.md`** (alongside
`docs/WORKLIST.md`). It is BOTH the memory buffer between rounds and the queue.
Item format + admission/dedup/budget rules: **`reference/worklist-schema.md`**.
Status markers mirror the main worklist: `[ ]` open · `[>]` in progress ·
`[✓]` done · `[blocked] <reason>`. Mutations go through plan-validate-execute:
write `changes.json` → `scripts/append-item.sh` validates + admits → only then it
lands.

## XCP farm command table (all builds/renders go here — never local)

| Need | Command |
|------|---------|
| confirm a slot | `./install-helpers/xcp-build.sh slots` |
| build a surface | `./install-helpers/xcp-build.sh cargo build -p <crate> --slot <s>` |
| full gates | `./install-helpers/xcp-build.sh gates --slot <s>` |
| token tests | `./install-helpers/xcp-build.sh crate mde-theme test --slot <s>` |
| render a surface | `./install-helpers/xcp-build.sh render '<slug>' --slot <s>` → `.xcp-build/renders/` |
| parallel gates | `./install-helpers/xcp-parallel.sh gates <s1> <s2>` |
| last result JSON | `./install-helpers/xcp-build.sh result latest` |

`render` builds + headless-renders `mde-workbench` (sway headless + grim, pixman
software). The launcher *dropdown* (`mde-cosmic-applet`) needs a panel host, so
its geometry is verified by the `parse_menu_size_from_kdl` unit tests + tokens
rather than a standalone render.

## Guardrails (the loop's discipline)

- **FIT GATE FIRST.** Only loop a finding that has a machine-checkable signal AND
  where iteration adds measured value (the `score-surface.sh` delta). No signal →
  flag it for the operator; don't churn on subjective calls. Default to a single
  well-prompted pass when that suffices.
- **EXTERNAL GROUNDING.** Accept/reject is decided by gates + lints +
  `score-surface.sh` + idle-CPU + the re-rendered PNG — never the model rating its
  own output. Keep the critique (evaluator) and improve (generator) steps distinct;
  one call never both authors and approves a change.
- **ONE CHANGE / REVERT-ON-NO-GAIN.** Exactly one edit per round; revert if the
  score doesn't improve or a gate fails; ≤3 retries then escalate.
- **ADMISSION + DEDUP + BUDGET.** New items only via `scripts/append-item.sh`
  (schema-valid, non-duplicate, over-threshold, under the global budget). Converts
  "generate forever" into bounded grounded progress.
- **OPERATOR CHECKPOINTS.** Pause before anything outward-facing/expensive (push,
  release, cutover are NEVER auto-fired); on a blocker; on a subjective call; and
  for a sampled human audit (present the before/after gallery).
- **SCOPE.** §4 (tokens, no raw hex/off-scale outside `mde-theme`), §6 (glue not
  reimplementation), §7 (done = builds + tests green + renders through tokens).

## NOT this skill

Single scoped UI edit → just do it. Functional backlog drain → `/ship`. One-off
render verify → `/preview`. Design/survey/worklist authoring before code →
`/plan`. Release cut → `/release`. Integrity sweep → `/audit`.
