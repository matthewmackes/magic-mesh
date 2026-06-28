# DRAIN-ENGINE — keep the build going until the worklist is clear

**Problem (operator, 2026-06-24):** *"Claude cannot be depended upon to continue to
act on / build the worklist. Create methods to keep Claude building until the worklist
is clear."*

Diagnosed from the session that prompted it, the stalls were never "hard work" — they
were **mechanical failures that halted all progress**:

1. **Disk wedges** — subagents ran *local* `cargo build` (often straying into the
   shared worktree), filling the dev-host disk to 100% **4×**, each wedging the whole
   session until manual recovery.
2. **Sequential grinding** — one big thing chased for many turns while the farm sat
   idle and the queue didn't move.
3. **Stalling on a blocked/gated item** instead of parking it and continuing.
4. **Flinching** on slow/fuzzy (GUI/infra) work.
5. **No durable keep-going machinery** — each turn re-derived the next step; when
   blocked, the loop *waited* instead of clearing the blocker.

## Locks (operator survey, 2026-06-24)

| # | Decision | Lock |
|---|---|---|
| L1 | Continuity engine | **Hybrid** — a fleet-side GitOps reconciler drains `@farm`-tagged worklist tasks mechanically (no AI tokens for routine builds); Claude **supervises**: design, failure-triage, gated/infra work, gap-fill. Realizes the §10 FARM-AUTO direction. |
| L2 | Guardrails | **Hard enforcement** — local builds are made *impossible*; disk pressure auto-heals. |
| L3 | Blocked tasks | **Park + keep draining** — a blocker becomes `[!]` + a surfaced `docs/NEEDS-OPERATOR.md` line, and the loop immediately moves to the next unblocked unit. It NEVER stalls on one item. |

## Architecture

### A. Hard guardrails (L2) — *implemented this session*
- **`install-helpers/cargo-farm-guard.sh`** — installed AS `cargo` (ahead of the real
  toolchain, preserved as `cargo-real`). `build`/`test`/`check`/`clippy`/`run`/… exit
  97 with "build on the farm"; `fmt`/`metadata`/… pass through. Makes the #1 stall
  (local-build disk wedge) **physically impossible**. `xcp-build.sh` is unaffected — it
  runs cargo on the farm over SSH, never through the shim.
- **`install-helpers/disk-watchdog.sh`** — reclaims dev-host disk when free `<` threshold
  (default 8G): drops stray `target/` dirs + aged task logs + prunes worktree admin.
  **Active-safe**: never removes a possibly-live agent's worktree (that is the
  coordinator's job *after* it merges that agent's PR).
- **`install-helpers/install-drain-guardrails.sh {--install|--uninstall}`** — operator-run
  (like `enable-autonomy.sh`): installs the guard + a 5-min `disk-watchdog` systemd timer.
  Reversible.
- **`install-helpers/check-worktree-isolation.sh`** (DRAIN-7) — the STEP-0 guard that an
  agent is in its OWN isolated worktree, not a shared checkout (the main `/root/magic-mesh`
  or the `bright-elm-ajw0` / `calm-ray-dcr8` worktrees). Run it (`./install-helpers/check-worktree-isolation.sh`,
  rc≠0 = refuse) or `source` it and call `require_isolated_worktree` before any edit; carries
  a `--self-test`. Makes the stray-into-shared-worktree failure (below) mechanically refusable.

### B. Hybrid engine (L1)
- **Mechanical half (no AI):** the FARM-AUTO GitOps reconciler — a timer reads the
  worklist for `@farm:{crate,verify}` tags on open units, builds the slice on the farm,
  opens a PR, and flips the task `[>]→needs-review`. No AI tokens in the build loop
  (governance §10 / `docs/design/build-platform.md`). *(DRAIN-4 — to build.)*
- **AI supervisor half:** Claude runs the **coordinator pattern** (below) for the work
  the reconciler can't do mechanically — design, triage of red builds, gated/infra/live
  work, and keeping the farm saturated when the reconciler is idle.

### C. Coordinator pattern (the AI loop that doesn't stall)
Every drain tick, in order:
1. **Pre-flight** `disk-watchdog.sh` (guarantee headroom before spawning).
2. **Saturate** — keep **N** farm-building agents in flight (N = free farm slots,
   spread per §10 caps), each on a disjoint crate, **farm-only builds** (the guard
   enforces it), each told to operate **only in its own worktree** (DRAIN-7).
3. **On each agent completion** (notification-driven): merge its PR (gates lifted),
   **reclaim its worktree**, and **immediately relaunch** a fresh agent on the next
   unit — never batch-wait.
4. **Park, don't stall** (L3): a unit that hits a live-infra/artifact/gate blocker →
   `[!]` + a `docs/NEEDS-OPERATOR.md` entry → continue with the next unit.
5. **No-flinch:** GUI/infra/gated units are picked on the same footing as backend; the
   measure is finished epics, not clean success signals.

## Acceptance (each runtime-observable)
- A local `cargo build` on the dev host exits non-zero with the farm redirect (guard live).
- Free disk on `/` never drops below the watchdog threshold for more than one tick.
- With the loop running, **N agents stay in flight** until the open+unblocked worklist
  is empty; a completed agent is merged + replaced within one tick (no idle farm while
  buildable units remain).
- A blocked unit appears in `docs/NEEDS-OPERATOR.md` and the loop continues — zero
  whole-loop stalls attributable to one item.
- The reconciler builds an `@farm`-tagged unit + opens a PR with no AI tokens spent.

## Risks
- The cargo guard is a system-level change — reversible (`--uninstall` restores
  `cargo-real`); `rustup` self-update could restore `cargo` (the timer re-asserts).
- Auto-merge under lifted gates can land a bad auto-merge across crates — mitigate with
  a periodic full-workspace integration gate on the farm (cheap, non-blocking).
- Worktree straying (DRAIN-7) silently lost an agent's work once — the fix makes the
  shared worktree off-limits to agents, enforced at STEP 0 by
  `install-helpers/check-worktree-isolation.sh` (refuses + exits non-zero on a shared/main checkout).

## Out of scope
- Replacing the operator-gated `/release` (RPM cut/publish stays deliberate).
- Auto-provisioning new paid infra without explicit authorization.
