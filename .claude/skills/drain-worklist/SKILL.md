---
name: drain-worklist
description: >-
  Drain docs/platform/WORKLIST.md by fanning disjoint parallel workers across
  the MCNF Xen build farm under the AI_GOVERNANCE.md §10.0.4 Parallel drain
  contract. TRIGGER when the operator says "drain the worklist", "drain
  worklist fully", "fan out", "utilize (the/full) farm", "use full farm
  capacity", "keep agents building", "parallel drain", "why are you not
  working on more epics", "farm is idle", or asks for progress across many
  epics at once. Each worker gets a disjoint write scope and runs its
  `@farm:{cargo …}` unit on an admitted farm slot; the parent folds
  completed workers, commits + pushes under standing auth, and re-fans
  against surfaced blockers on every tick. NOT for a single scoped edit
  (just do it), design polish (use polish), or a release cut (use ship).
---

# drain-worklist — parallel drain of the MCNF platform worklist

Durable authority: **`AI_GOVERNANCE.md` §10.0.4 (Parallel drain contract,
2026-08-20)**. Shared operational glue: **`AGENTS.md` → Parallel Drain
Contract**. Story format + retrofit: **`docs/platform/WORKLIST.md` → Parallel
drain execution contract**. Re-read all three at the start of every tick;
the newest lock wins.

The DRAIN-ENGINE was built by earlier agents and is preserved in-tree. This
skill wires Claude Code into that engine — **do not reinvent a scheduler,
queue, or slot map.**

Claude Code owns this invocation's implementation fan-out. Set
`MCNF_AGENT_RUNTIME=claude` and use a Claude-native
`MCNF_AGENT_DISPATCHER`; never dispatch Cursor or Codex workers from a Claude
drain. The bridge fails closed when the native adapter is absent.

## What actually holds the farm parallel

The scheduler chain reads `@farm:{cargo …}` payloads out of `Status:
Remaining` epics via `automation/lib/farm-jobs.sh active`, so if the
worklist carries none, the queue is empty by construction and 10 heavy
farm slots sit idle even though the machinery is running. Under §10.0.4:

- Every `Status: Remaining` epic MUST carry ≥1 real `@farm:{cargo …}`
 payload. Canonical carrier: the `Verification method:` field. Multiple
 payloads authorise disjoint parallel workers.
- `install-helpers/lint-worklist.sh` fails commits that violate this.
- A non-`cargo …` payload is a template and contributes zero demand.

## Farm fill control (shared; do not fork)

The farm is filled by `automation/reconciler/farm-reconcile.sh` via
`mcnf-farm-reconcile.service`. Two triggers start the same oneshot:

- `mcnf-farm-reconcile.timer` — every 15 minutes
- `mcnf-farm-reconcile.path` — on `.git/logs/HEAD` / `.git/HEAD` change

`is_fresh` skips a job whose result `commit` equals a clean HEAD. After a
commit the previous SHA is stale, but the 15-minute timer may still be
waiting — that looks like an idle farm. Do not treat `fresh @ <old SHA>`
or `nothing to do — farm converged` at the *previous* HEAD as farm-down.

After every commit/push:

```bash
automation/reconciler/tick-fill.sh
# equivalent: systemctl start --no-block mcnf-farm-reconcile.service
```

Do not hand-fan `xcp-build.sh` of a command the reconciler already owns
(`docs/BUILD-ENVIRONMENT.md` §4A.5). Do not grind `cargo test --workspace`
as filler unless that command is the epic's official unit. If every unique
cargo command is already fresh at this clean HEAD and Remaining leftovers
are dest/live/seat work, idle slots are correct — fan implementation
agents, not cargo. Do not run a live `reconciler-up.sh` install on
`rocky9-kvm2` just to fill slots.

## Tick recipe (run in this order every session)

1. **Re-read the locks.** `AI_GOVERNANCE.md §10.0.4`, `AGENTS.md`,
 `docs/platform/WORKLIST.md`.
2. **Probe live farm.** `./install-helpers/farm-topology.sh check` (must
 report N/5 dom0s up and free-slot totals).
3. **Read demand.** `./automation/lib/farm-jobs.sh active | wc -l`. If
 zero and Remaining epics exist, the first act is decomposing the
 top-priority Remaining epic into disjoint `@farm:{cargo …}` units —
 not single-threaded implementation.
4. **Plan the tick.** `./install-helpers/drain-coordinator.sh plan <N>`
 (per-node free slot map + next-N candidate units).
5. **Fan out.** Spawn one background worker per disjoint unit. Give each
 worker:
 - the exact epic ID and its `@farm:{cargo …}` command,
 - the concrete file scope it may edit (from the epic's `Relevant
 files/components` intersected with the unit),
 - a hard "do not touch files owned by peer workers" rule,
 - farm-execute-only mandate (local cargo build/test/clippy is blocked
 by `install-helpers/cargo-farm-guard.sh` with exit 97),
 - explicit farm slot suggestion (long pole → BigBoy `.130`).
6. **Keep working locally** on non-conflicting critical-path work while
 workers run. Do not block on their result unless the next step
 requires it.
7. **Fold completions** as they return. Commit (with standing auth)
 grouping cohesive lanes; push. Preserve other agents' dirty edits.
 Then `automation/reconciler/tick-fill.sh` — do not wait for the timer.
8. **Re-fan against surfaced blockers.** A worker that hits a
 pre-existing farm-wide blocker (strict-clippy drift, style-leak lint
 failure, farm-side infra breakage) surfaces it as the next fan-out
 target; do not silently retry.
9. **Park, don't stall.** A blocked epic goes through
 `automation/drain/park-worklist-item.sh` so its lane parks without
 stopping the drain.
10. **Never invent a second scheduler.** All parallel-execution
 primitives live in-tree; extend them by contribution, not by
 duplication.

## Canonical helper inventory (reuse, don't rebuild)

| Helper | Purpose |
|---|---|
| `install-helpers/farm-topology.sh` | one roster (5 dom0s / 10 heavy slots) |
| `install-helpers/drain-coordinator.sh` | per-tick free-slot + candidate map |
| `automation/drain/ship-coordinator.sh` | control-host tick (preflight + reconcile + review surfaces) |
| `automation/lib/farm-jobs.sh` | worklist → queue |
| `automation/queue/farm-enqueue.sh` | etcd `/farm/queue/*` push |
| `automation/queue/farm-agent.sh` | slot consumer |
| `automation/reconciler/farm-reconcile.sh` | converge `@farm` jobs onto slots |
| `automation/reconciler/tick-fill.sh` | start the fill oneshot after commit |
| `mcnf-farm-reconcile.{timer,path}` | 15-min timer + HEAD-change trigger |
| `install-helpers/farm-reconciler.sh` | build-VM autoscale from queue demand |
| `install-helpers/farm-slot-gc.sh` | 20-min stale-slot GC (fleet install with `--deploy`) |
| `install-helpers/cargo-farm-guard.sh` + `install-drain-guardrails.sh` | local heavy cargo hard-blocked |
| `automation/drain/park-worklist-item.sh` | park a blocker without stalling the loop |
| `install-helpers/xcp-build.sh` | heavy build/test on admitted farm slots |
| `install-helpers/farm-slot-gc.sh --deploy` | fleet-wide GC install |
| `install-helpers/lint-worklist.sh` | §10.0.4 gate + full worklist lint |

## Farm roster (do not hardcode)

Source of truth: `install-helpers/farm-topology.sh`. Current shape (2026-08-20):

| Node | Role | Cap | Notes |
|---|---|---|---|
| `.50` XEN-HOME-SERVICES | small | 2 | fmt, focused tests, small crates |
| `.90` KVM-XCP1 | small | 2 | focused tests |
| `.130` XEN-BIGBOY | long pole | 3 | workspace, cold GUI, RPM, strict clippy |
| `.170` XEN-194 | small | 2 | focused tests |
| `.196` XEN-196 | small | 1 | packaging checks, format gate |

Long pole always routes to BigBoy first.

## Definition of done for a tick

- Every free heavy slot the tick was allowed to consume ran a real
 `@farm:{cargo …}` unit, or the queue was legitimately smaller than the
 free slot count.
- Completed workers were folded, committed, and pushed under standing
 auth (never rely on the operator to babysit that).
- Any blocker surfaced was either fixed or parked with a follow-up epic.
- `install-helpers/lint-worklist.sh` still passes; no `Status: Remaining`
 epic was left without a real `@farm:{cargo …}` payload.
- Next-tick plan is stated (what will fan out next, and why).

## Anti-patterns (governance violations)

- **Serial serialization.** Working one epic end-to-end while free slots
 stand idle. Fan out instead.
- **Prose payloads.** `@farm:{crate,verify}` / `@farm:{…}` do not count.
 Only `cargo …` payloads reach the queue.
- **Bypassing the cargo guard.** Do not `PATH=`-shim past
 `cargo-farm-guard.sh`. Use `install-helpers/xcp-build.sh cargo …`.
- **Silent ENOSPC retry.** ENOSPC after admission is a capacity incident
 (§10.0.3.5), not a routine retry. Park + report the slot.
- **New scheduler surface.** The engine exists. Extend it; do not fork
 it.
- **Waiting on the 15-min timer.** After HEAD moves, start the oneshot.
- **Filler workspace grind.** Official REL-007 unit is the only
 workspace grind. Fresh cargo at this HEAD plus dest/live leftovers
 is implementation work.

## Related skills

- `.claude/skills/polish/SKILL.md` — GUI-only refinement loop (this
 skill's aesthetic counterpart).

## When NOT to use

- A single, small, in-flight edit that is faster to just do.
- Design polish that belongs to `polish`.
- A release cut / candidate build (that has its own coordinator epic).
